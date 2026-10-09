//! Bounded report primitives. krilla owns PDF/font encoding and subsetting;
//! no HTML, external images, file URLs, JavaScript or font paths are accepted.
use krilla::{
    color::rgb,
    geom::{PathBuilder, Point},
    page::PageSettings,
    paint::{Fill, Stroke},
    text::{Font, TextDirection},
    Document,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::PathBuf, time::Instant};

const MAX_SPEC_BYTES: usize = 512 * 1024;
const MAX_FONT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 128 * 1024;
const MAX_PAGES: usize = 24;
const MAX_CHARTS: usize = 16;
const MAX_CHART_VALUES: usize = 8192;
const PAGE_W: f32 = 595.28;
const PAGE_H: f32 = 841.89;
const MARGIN: f32 = 42.0;
const COLORS: [[u8; 3]; 8] = [
    [38, 94, 172],
    [214, 111, 39],
    [46, 133, 96],
    [149, 82, 159],
    [201, 69, 80],
    [44, 137, 157],
    [153, 125, 46],
    [97, 109, 130],
];

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChartSpec {
    #[serde(rename = "type")]
    kind: String,
    title: String,
    labels: Vec<String>,
    series: Vec<Series>,
    #[serde(default, rename = "shareBasis", skip_serializing_if = "Option::is_none")]
    share_basis: Option<ShareBasis>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Series {
    name: String,
    values: Vec<f64>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShareBasis {
    scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    total: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    other_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<ShareSource>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShareSource {
    input_id: String,
    sheet: String,
    total_column: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label_column: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportSpec {
    title: String,
    blocks: Vec<Block>,
}
#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Block {
    #[serde(rename = "paragraph")]
    Paragraph { text: String },
    #[serde(rename = "heading")]
    Heading { text: String },
    #[serde(rename = "table")]
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
    },
    #[serde(rename = "chart")]
    Chart { chart: ChartSpec },
    #[serde(rename = "pageBreak")]
    PageBreak,
}

fn invalid(message: impl std::fmt::Display) -> String {
    format!("[tool.invalid_input] report: {message}")
}
fn check(cancelled: &dyn Fn() -> bool, deadline: Instant) -> Result<(), String> {
    if cancelled() {
        return Err("[tool.cancelled] report generation cancelled".into());
    }
    if Instant::now() >= deadline {
        return Err("[tool.computation_timed_out] report generation timed out".into());
    }
    Ok(())
}
fn text_bound(text: &str, limit: usize) -> Result<(), String> {
    if text.len() > limit || text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(invalid(
            "text is over its limit or contains control characters",
        ));
    }
    Ok(())
}
fn chart_validate(chart: &ChartSpec) -> Result<(), String> {
    if !matches!(chart.kind.as_str(), "line" | "bar" | "pie") {
        return Err(invalid("chart type must be line, bar or pie"));
    }
    text_bound(&chart.title, 160)?;
    if chart.labels.is_empty()
        || chart.labels.len() > 256
        || chart.series.is_empty()
        || chart.series.len() > 8
    {
        return Err(invalid("charts need 1..256 labels and 1..8 series"));
    }
    for label in &chart.labels {
        text_bound(label, 96)?;
    }
    for series in &chart.series {
        text_bound(&series.name, 96)?;
        if series.values.len() != chart.labels.len()
            || series
                .values
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 1e12)
        {
            return Err(invalid(
                "each series needs one finite value per label, within +/-1e12",
            ));
        }
    }
    if chart.kind == "pie" {
        if chart.series.len() != 1
            || chart.labels.len() > 12
            || chart.series[0].values.iter().any(|v| *v < 0.0)
        {
            return Err(invalid(
                "pie charts need one series, up to 12 labels and nonnegative values",
            ));
        }
        let overall_remainder=chart.share_basis.as_ref().is_some_and(|basis|basis.scope=="overall" && basis.total.is_some_and(|total|total.is_finite() && total>0.0));
        if chart.series[0].values.iter().sum::<f64>() <= 0.0 && !overall_remainder {
            return Err(invalid("pie total must be positive"));
        }
    }
    if let Some(basis) = &chart.share_basis {
        if chart.kind != "pie" || !matches!(basis.scope.as_str(), "subset" | "overall") {
            return Err(invalid("shareBasis is supported only for pie charts with subset or overall scope"));
        }
        if let Some(label) = &basis.other_label { text_bound(label, 96)?; if label.trim().is_empty() { return Err(invalid("otherLabel must not be empty")); } }
        if let Some(source) = &basis.source {
            for value in [&source.input_id, &source.sheet, &source.total_column] {
                text_bound(value, 256)?;
                if value.trim().is_empty() { return Err(invalid("shareBasis source identifiers must not be empty")); }
            }
            if let Some(label) = &source.label_column { text_bound(label, 256)?; if label.trim().is_empty() {return Err(invalid("source labelColumn must not be empty"));} }
        }
        let subtotal = chart.series[0].values.iter().sum::<f64>();
        if !subtotal.is_finite() {return Err(invalid("pie sum is not finite"));}
        if let Some(total) = basis.total {
            if !total.is_finite() || total <= 0.0 {return Err(invalid("shareBasis total must be finite and positive"));}
            let tolerance = total.abs() * (32.0 * f64::EPSILON);
            if subtotal > total + tolerance || basis.scope == "subset" && (subtotal - total).abs() > tolerance {
                return Err(invalid("shareBasis total differs from its declared scope"));
            }
        } else if basis.scope == "overall" {return Err(invalid("overall shareBasis requires an explicit total"));}
    }
    Ok(())
}

/// Source is a binding request only; the renderer has no input-file authority.
fn prepare_chart(mut chart: ChartSpec) -> Result<(ChartSpec, Value), String> {
    chart_validate(&chart)?;
    let original = chart.clone();
    let subtotal = chart.series.first().map(|series|series.values.iter().sum::<f64>()).unwrap_or(0.0);
    if let Some(basis) = &chart.share_basis {
        if basis.scope == "overall" {
            let total = basis.total.ok_or_else(||invalid("overall shareBasis requires total"))?;
            let remaining = total - subtotal;
            let tolerance=total.abs() * (32.0 * f64::EPSILON);
            if remaining > tolerance || subtotal==0.0 && remaining>0.0 {
                if chart.labels.len() >= 12 {return Err(invalid("overall pie needs room for its other category within 12 labels"));}
                let other = basis.other_label.clone().unwrap_or_else(||"其他".into());
                if chart.labels.iter().any(|label|label==&other) {return Err(invalid("otherLabel conflicts with an existing label"));}
                chart.labels.push(other);
                chart.series[0].values.push(remaining);
            }
        }
    }
    chart_validate(&chart)?;
    if chart.kind=="pie" && (!chart.series[0].values.iter().sum::<f64>().is_finite() || chart.series[0].values.iter().sum::<f64>()<=0.0) {
        return Err(invalid("prepared pie needs a finite positive rendered sum"));
    }
    let share = original.share_basis.as_ref().map(|basis|json!(basis)).or_else(||(chart.kind=="pie").then(||json!({"scope":"subset","total":subtotal})));
    let mut contract = json!({"type":original.kind,"title":original.title,"labels":original.labels,
        "series":original.series,"shareBasis":share,"sourceVerified":false});
    if chart.labels != original.labels {
        contract["renderedLabels"]=json!(chart.labels);
        contract["renderedSeries"]=json!(chart.series);
    }
    if chart.kind=="pie" {contract["renderedTotal"]=json!(chart.series[0].values.iter().sum::<f64>());}
    Ok((chart,contract))
}

struct SystemFont {
    bytes: Vec<u8>,
    name: String,
    digest: String,
}
fn embedding_allowed(permissions: Option<ttf_parser::Permissions>, subset: bool, outline: bool) -> bool {
    matches!(permissions, Some(ttf_parser::Permissions::Installable | ttf_parser::Permissions::Editable)) && subset && outline
}
fn system_font() -> Result<SystemFont, String> {
    // Fixed OS candidates only. No model-provided path and no redistribution.
    #[cfg(windows)]
    let candidates = ["msyh.ttc", "msyh.ttf", "simhei.ttf"];
    #[cfg(not(windows))]
    let candidates: [&str; 0] = [];
    let root = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()))
        .join("Fonts");
    for name in candidates {
        let path = root.join(name);
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                continue;
            }
        }
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_FONT_BYTES
        {
            continue;
        }
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file
            .take(MAX_FONT_BYTES + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > MAX_FONT_BYTES as usize
        {
            continue;
        }
        let Ok(face) = ttf_parser::Face::parse(&bytes, 0) else {
            continue;
        };
        if !embedding_allowed(face.permissions(), face.is_subsetting_allowed(), face.is_outline_embedding_allowed())
            || face.glyph_index('中').is_none()
        {
            continue;
        }
        let digest = hex_digest(&bytes);
        return Ok(SystemFont {
            bytes,
            name: name.into(),
            digest,
        });
    }
    Err("[tool.pdf_font_unavailable] No installed CJK font permits editable document embedding and subsetting; PDF generation is unavailable on this machine".into())
}
fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub(crate) fn availability() -> Value {
    match system_font() {
        Ok(font) => {
            json!({"availability":"available","renderer":"krilla-0.8.2","font":font.name,"fontSha256":font.digest,
            "fontEmbedding":"editable-or-installable/subset/outline","maxPages":MAX_PAGES,"maxSpecBytes":MAX_SPEC_BYTES,"chartTypes":["line","bar","pie"]})
        }
        Err(error) => {
            json!({"availability":"unavailable","reasonCode":"tool.pdf_font_unavailable","reason":error})
        }
    }
}

#[derive(Clone)]
enum Draw {
    Text {
        x: f32,
        y: f32,
        size: f32,
        text: String,
        color: [u8; 3],
    },
    Path {
        points: Vec<(f32, f32)>,
        close: bool,
        fill: Option<[u8; 3]>,
        stroke: Option<[u8; 3]>,
    },
}
fn text(draws: &mut Vec<Draw>, x: f32, y: f32, size: f32, value: impl Into<String>) {
    draws.push(Draw::Text {
        x,
        y,
        size,
        text: value.into(),
        color: [32, 40, 52],
    });
}
fn line(draws: &mut Vec<Draw>, points: Vec<(f32, f32)>, color: [u8; 3]) {
    draws.push(Draw::Path {
        points,
        close: false,
        fill: None,
        stroke: Some(color),
    });
}
fn rect(draws: &mut Vec<Draw>, x: f32, y: f32, w: f32, h: f32, color: [u8; 3]) {
    draws.push(Draw::Path {
        points: vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)],
        close: true,
        fill: Some(color),
        stroke: None,
    });
}
fn compact_label(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let short: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{short}...")
    } else {
        short
    }
}
fn chart_draws(chart: &ChartSpec, x: f32, y: f32, width: f32, height: f32) -> Vec<Draw> {
    let mut draws = Vec::new();
    let mut title = String::new();
    let mut used = 0.0f32;
    let mut title_lines = 0;
    for c in chart.title.chars() {
        let advance = if c.is_ascii() { 8.4 } else { 14.0 };
        if used + advance > width && !title.is_empty() {
            text(
                &mut draws,
                x,
                y + 20.0 + title_lines as f32 * 18.0,
                14.0,
                std::mem::take(&mut title),
            );
            title_lines += 1;
            used = 0.0;
        }
        title.push(c);
        used += advance;
    }
    text(
        &mut draws,
        x,
        y + 20.0 + title_lines as f32 * 18.0,
        14.0,
        title,
    );
    if chart.kind == "pie" {
        let total = chart.series[0].values.iter().sum::<f64>();
        let cx = x + width * 0.30;
        let cy = y + height * 0.52;
        let radius = height * 0.32;
        let mut start = -std::f64::consts::FRAC_PI_2;
        for (i, value) in chart.series[0].values.iter().enumerate() {
            let sweep = value / total * std::f64::consts::TAU;
            let steps = (sweep.abs() * 24.0).ceil().max(1.0) as usize;
            let mut points = vec![(cx, cy)];
            for k in 0..=steps {
                let a = start + sweep * k as f64 / steps as f64;
                points.push((cx + radius * a.cos() as f32, cy + radius * a.sin() as f32));
            }
            draws.push(Draw::Path {
                points,
                close: true,
                fill: Some(COLORS[i % 8]),
                stroke: Some([255, 255, 255]),
            });
            let legend_step =
                ((height - 80.0 - title_lines as f32 * 18.0) / chart.labels.len() as f32).min(22.0);
            let ly = y + 60.0 + title_lines as f32 * 18.0 + i as f32 * legend_step;
            rect(
                &mut draws,
                x + width * 0.61,
                ly - 9.0,
                9.0,
                9.0,
                COLORS[i % 8],
            );
            text(
                &mut draws,
                x + width * 0.61 + 15.0,
                ly,
                10.0,
                format!(
                    "{} {:.1}%",
                    compact_label(&chart.labels[i], 14),
                    value / total * 100.0
                ),
            );
            start += sweep;
        }
        return draws;
    }
    let left = x + 62.0;
    let top = y + 52.0 + title_lines as f32 * 18.0;
    let plot_w = width - 78.0;
    let plot_h = height - 126.0 - title_lines as f32 * 18.0;
    let (mut low, mut high) = (0.0f64, 0.0f64);
    for value in chart.series.iter().flat_map(|s| &s.values) {
        low = low.min(*value);
        high = high.max(*value);
    }
    if high == low {
        high = low + 1.0;
    }
    let py = |v: f64| top + plot_h - ((v - low) / (high - low)) as f32 * plot_h;
    for k in 0..=4 {
        let v = low + (high - low) * k as f64 / 4.0;
        let yy = py(v);
        line(
            &mut draws,
            vec![(left, yy), (left + plot_w, yy)],
            [222, 228, 234],
        );
        text(&mut draws, x, yy + 3.0, 9.0, format!("{v:.1}"));
    }
    line(
        &mut draws,
        vec![
            (left, top),
            (left, top + plot_h),
            (left + plot_w, top + plot_h),
        ],
        [100, 111, 125],
    );
    let step = plot_w / chart.labels.len() as f32;
    let stride = chart.labels.len().div_ceil(8).max(1);
    for (i, label) in chart
        .labels
        .iter()
        .enumerate()
        .filter(|(i, _)| i % stride == 0)
    {
        text(
            &mut draws,
            left + step * (i as f32 + 0.5) - 18.0,
            top + plot_h + 18.0,
            8.0,
            compact_label(label, 10),
        );
    }
    for (s, series) in chart.series.iter().enumerate() {
        if chart.kind == "line" {
            let points = series
                .values
                .iter()
                .enumerate()
                .map(|(i, v)| (left + step * (i as f32 + 0.5), py(*v)))
                .collect::<Vec<_>>();
            line(&mut draws, points.clone(), COLORS[s]);
            for (px, py) in points {
                rect(&mut draws, px - 1.8, py - 1.8, 3.6, 3.6, COLORS[s]);
            }
        } else {
            let bar_w = step * 0.8 / chart.series.len() as f32;
            for (i, v) in series.values.iter().enumerate() {
                let yy = py(*v);
                let zero = py(0.0);
                rect(
                    &mut draws,
                    left + step * (i as f32 + 0.1) + s as f32 * bar_w,
                    yy.min(zero),
                    bar_w * 0.9,
                    (yy - zero).abs().max(0.3),
                    COLORS[s],
                );
            }
        }
        let lx = x + (s % 4) as f32 * (width / 4.0);
        let ly = y + height - 22.0 + (s / 4) as f32 * 15.0;
        rect(&mut draws, lx, ly - 9.0, 9.0, 9.0, COLORS[s]);
        text(
            &mut draws,
            lx + 14.0,
            ly,
            9.0,
            compact_label(&series.name, 16),
        );
    }
    draws
}
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn svg_color(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}
pub(crate) struct ChartOutput { pub bytes: Vec<u8>, pub data_contract: Value }
pub(crate) fn render_chart(value: &Value) -> Result<Vec<u8>, String> {
    render_chart_output(value).map(|output|output.bytes)
}
pub(crate) fn render_chart_output(value: &Value) -> Result<ChartOutput, String> {
    if serde_json::to_vec(value).map_err(invalid)?.len() > MAX_SPEC_BYTES {
        return Err(invalid("chart specification exceeds 512 KiB"));
    }
    let chart: ChartSpec = serde_json::from_value(value.clone()).map_err(invalid)?;
    let (chart,contract)=prepare_chart(chart)?;
    let mut svg=String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"800\" height=\"480\" viewBox=\"0 0 800 480\"><rect width=\"800\" height=\"480\" fill=\"white\"/>");
    for draw in chart_draws(&chart, 20.0, 10.0, 760.0, 450.0) {
        match draw {
            Draw::Text{x,y,size,text,color}=>svg.push_str(&format!("<text x=\"{x}\" y=\"{y}\" font-size=\"{size}\" font-family=\"Microsoft YaHei,sans-serif\" fill=\"{}\">{}</text>",svg_color(color),xml(&text))),
            Draw::Path{points,close,fill,stroke}=>{let points=points.iter().map(|(x,y)|format!("{x},{y}")).collect::<Vec<_>>().join(" ");
                let element=if close {"polygon"} else {"polyline"};
                svg.push_str(&format!("<{element} points=\"{points}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\"/>",fill.map(svg_color).unwrap_or("none".into()),stroke.map(svg_color).unwrap_or("none".into())));},
        }
    }
    svg.push_str("</svg>");
    Ok(ChartOutput {bytes:svg.into_bytes(),data_contract:json!({"schemaVersion":1,"kind":"chart","sourceVerified":false,"charts":[contract]})})
}

struct Layout<'a> {
    pages: Vec<Vec<Draw>>,
    y: f32,
    face: ttf_parser::Face<'a>,
    text_bytes: usize,
}
impl<'a> Layout<'a> {
    fn new(face: ttf_parser::Face<'a>) -> Self {
        Self {
            pages: vec![Vec::new()],
            y: MARGIN,
            face,
            text_bytes: 0,
        }
    }
    fn page(&mut self) -> Result<(), String> {
        if self.pages.len() >= MAX_PAGES {
            return Err(invalid("report exceeds 24 pages"));
        }
        self.pages.push(Vec::new());
        self.y = MARGIN;
        Ok(())
    }
    fn space(&mut self, height: f32) -> Result<(), String> {
        if self.y + height > PAGE_H - MARGIN {
            self.page()?;
        }
        Ok(())
    }
    fn lines(&mut self, value: &str, size: f32, width: f32) -> Result<Vec<String>, String> {
        text_bound(value, 32 * 1024)?;
        self.text_bytes += value.len();
        if self.text_bytes > MAX_TEXT_BYTES {
            return Err(invalid("report text exceeds 128 KiB"));
        }
        let mut lines = Vec::new();
        let mut current = String::new();
        let mut used = 0.0;
        for c in value.chars() {
            if c == '\n' {
                lines.push(std::mem::take(&mut current));
                used = 0.0;
                continue;
            }
            let glyph = self.face.glyph_index(c).ok_or_else(|| {
                format!(
                    "[tool.pdf_missing_glyph] System report font does not contain U+{:04X}",
                    c as u32
                )
            })?;
            let advance = self
                .face
                .glyph_hor_advance(glyph)
                .unwrap_or(self.face.units_per_em()) as f32
                / self.face.units_per_em() as f32
                * size;
            if used + advance > width && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                used = 0.0;
            }
            current.push(c);
            used += advance;
        }
        if !current.is_empty() || lines.is_empty() {
            lines.push(current);
        }
        Ok(lines)
    }
    fn paragraph(&mut self, value: &str, size: f32) -> Result<(), String> {
        for value in self.lines(value, size, PAGE_W - 2.0 * MARGIN)? {
            self.space(size * 1.6)?;
            self.y += size * 1.5;
            text(self.pages.last_mut().unwrap(), MARGIN, self.y, size, value);
        }
        self.y += 9.0;
        Ok(())
    }
    fn table_row(&mut self, cells: &[String], header: bool) -> Result<(), String> {
        let width = (PAGE_W - 2.0 * MARGIN) / cells.len() as f32;
        let lines = cells
            .iter()
            .map(|s| self.lines(s, 9.0, width - 12.0))
            .collect::<Result<Vec<_>, _>>()?;
        let height = lines.iter().map(Vec::len).max().unwrap_or(1) as f32 * 14.0 + 12.0;
        if height > PAGE_H - 2.0 * MARGIN {
            return Err(invalid("table row is too tall for a page"));
        }
        self.space(height)?;
        let y = self.y;
        let draws = self.pages.last_mut().unwrap();
        if header {
            rect(
                draws,
                MARGIN,
                y,
                PAGE_W - 2.0 * MARGIN,
                height,
                [231, 238, 247],
            );
        }
        for (i, rows) in lines.iter().enumerate() {
            for (j, value) in rows.iter().enumerate() {
                text(
                    draws,
                    MARGIN + i as f32 * width + 6.0,
                    y + 15.0 + j as f32 * 14.0,
                    9.0,
                    value,
                );
            }
        }
        line(
            draws,
            vec![(MARGIN, y + height), (PAGE_W - MARGIN, y + height)],
            [211, 219, 227],
        );
        self.y += height;
        Ok(())
    }
}

pub(crate) struct PdfOutput {
    pub bytes: Vec<u8>,
    pub pages: usize,
    pub font_name: String,
    pub font_sha256: String,
    pub data_contract: Value,
}
pub(crate) fn render_pdf(
    value: &Value,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<PdfOutput, String> {
    check(cancelled, deadline)?;
    if serde_json::to_vec(value).map_err(invalid)?.len() > MAX_SPEC_BYTES {
        return Err(invalid("PDF specification exceeds 512 KiB"));
    }
    let spec: ReportSpec = serde_json::from_value(value.clone()).map_err(invalid)?;
    text_bound(&spec.title, 160)?;
    if spec.blocks.is_empty() || spec.blocks.len() > 128 {
        return Err(invalid("reports need 1..128 blocks"));
    }
    let system = system_font()?;
    check(cancelled, deadline)?;
    let face = ttf_parser::Face::parse(&system.bytes, 0).map_err(invalid)?;
    let mut layout = Layout::new(face);
    layout.paragraph(&spec.title, 20.0)?;
    let mut chart_count = 0;
    let mut chart_values = 0;
    let mut chart_contracts = Vec::new();
    for block in spec.blocks {
        check(cancelled, deadline)?;
        match block {
            Block::Paragraph { text } => layout.paragraph(&text, 11.0)?,
            Block::Heading { text } => {
                text_bound(&text, 160)?;
                layout.space(45.0)?;
                layout.paragraph(&text, 15.0)?;
            }
            Block::PageBreak => layout.page()?,
            Block::Table { columns, rows } => {
                if columns.is_empty() || columns.len() > 12 || rows.len() > 2000 {
                    return Err(invalid("tables need 1..12 columns and up to 2000 rows"));
                }
                layout.table_row(&columns, true)?;
                for row in rows {
                    check(cancelled, deadline)?;
                    if row.len() != columns.len() {
                        return Err(invalid("table row length differs from columns"));
                    }
                    let cells = row
                        .iter()
                        .map(|v| match v {
                            Value::String(s) => Ok(s.clone()),
                            Value::Number(n) => Ok(n.to_string()),
                            Value::Null => Ok(String::new()),
                            Value::Bool(b) => Ok(b.to_string()),
                            _ => Err(invalid("table cells must be scalar")),
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    layout.table_row(&cells, false)?;
                }
                layout.y += 12.0;
            }
            Block::Chart { chart } => {
                let (chart,contract)=prepare_chart(chart)?;
                chart_contracts.push(contract);
                chart_count += 1;
                chart_values += chart.labels.len() * chart.series.len();
                if chart_count > MAX_CHARTS || chart_values > MAX_CHART_VALUES {
                    return Err(invalid("report exceeds 16 charts or 8192 chart values"));
                }
                // Validate every label including labels that the visual tick stride omits.
                for label in chart
                    .labels
                    .iter()
                    .chain(chart.series.iter().map(|s| &s.name))
                    .chain(std::iter::once(&chart.title))
                {
                    layout.lines(label, 10.0, 500.0)?;
                }
                layout.space(340.0)?;
                layout.pages.last_mut().unwrap().extend(chart_draws(
                    &chart,
                    MARGIN,
                    layout.y,
                    PAGE_W - 2.0 * MARGIN,
                    320.0,
                ));
                layout.y += 340.0;
            }
        }
    }
    let pages = layout.pages.len();
    let page_draws = layout.pages;
    drop(layout.face);
    let font = Font::new(system.bytes.into(), 0)
        .ok_or("[tool.pdf_font_invalid] PDF library rejected the installed font")?;
    let mut document = Document::new();
    for (i, draws) in page_draws.into_iter().enumerate() {
        check(cancelled, deadline)?;
        let mut page = document.start_page_with(
            PageSettings::from_wh(PAGE_W, PAGE_H)
                .ok_or_else(|| invalid("invalid page dimensions"))?,
        );
        let mut surface = page.surface();
        let mut draws = draws;
        text(
            &mut draws,
            MARGIN,
            PAGE_H - 22.0,
            8.0,
            format!("{} / {pages}", i + 1),
        );
        for draw in draws {
            match draw {
                Draw::Text {
                    x,
                    y,
                    size,
                    text,
                    color,
                } => {
                    surface.set_stroke(None);
                    surface.set_fill(Some(Fill {
                        paint: rgb::Color::new(color[0], color[1], color[2]).into(),
                        ..Default::default()
                    }));
                    surface.draw_text(
                        Point::from_xy(x, y),
                        font.clone(),
                        size,
                        &text,
                        false,
                        TextDirection::Auto,
                    );
                }
                Draw::Path {
                    points,
                    close,
                    fill,
                    stroke,
                } => {
                    let mut pb = PathBuilder::new();
                    for (i, (x, y)) in points.iter().enumerate() {
                        if i == 0 {
                            pb.move_to(*x, *y);
                        } else {
                            pb.line_to(*x, *y);
                        }
                    }
                    if close {
                        pb.close();
                    }
                    if let Some(path) = pb.finish() {
                        surface.set_fill(fill.map(|c| Fill {
                            paint: rgb::Color::new(c[0], c[1], c[2]).into(),
                            ..Default::default()
                        }));
                        surface.set_stroke(stroke.map(|c| Stroke {
                            paint: rgb::Color::new(c[0], c[1], c[2]).into(),
                            width: 1.2,
                            ..Default::default()
                        }));
                        surface.draw_path(&path);
                    }
                }
            }
        }
        surface.finish();
        page.finish();
    }
    check(cancelled, deadline)?;
    let bytes = document
        .finish()
        .map_err(|e| format!("[tool.pdf_generation_failed] PDF serialization failed: {e:?}"))?;
    check(cancelled, deadline)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(invalid("generated PDF exceeds 8 MiB"));
    }
    Ok(PdfOutput {
        bytes,
        pages,
        font_name: system.name,
        font_sha256: system.digest,
        data_contract: json!({"schemaVersion":1,"kind":"pdf","sourceVerified":false,"charts":chart_contracts}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // These cases are written for follow-up verification and are not run here.
    #[test]
    fn r3b_overall_pie_renders_remainder_and_keeps_source_unverified() {
        let chart=json!({"type":"pie","title":"合成份额","labels":["A","B"],"series":[{"name":"count","values":[20,30]}],
            "shareBasis":{"scope":"overall","total":100,"otherLabel":"其余","source":{"inputId":"a","sheet":"Data","totalColumn":"Count","labelColumn":"Name"}}});
        let output=render_chart_output(&chart).unwrap();let svg=String::from_utf8(output.bytes).unwrap();
        assert!(svg.contains("20.0%") && svg.contains("30.0%") && svg.contains("50.0%"));
        assert_eq!(output.data_contract["charts"][0]["renderedLabels"],json!(["A","B","其余"]));
        assert_eq!(output.data_contract["sourceVerified"],false);
        assert_eq!(output.data_contract["charts"][0]["shareBasis"]["source"]["inputId"],"a");
    }
    #[test]
    fn r3b_default_subset_is_compatible_and_invalid_totals_are_rejected() {
        let chart=json!({"type":"pie","title":"subset","labels":["A","B"],"series":[{"name":"count","values":[20,30]}]});
        let svg=String::from_utf8(render_chart(&chart).unwrap()).unwrap();
        assert!(svg.contains("40.0%") && svg.contains("60.0%"));
        for basis in [json!({"scope":"overall"}),json!({"scope":"overall","total":40}),json!({"scope":"overall","total":-1}),json!({"scope":"subset","total":100})] {
            let mut invalid=chart.clone();invalid["shareBasis"]=basis;assert!(render_chart(&invalid).is_err());
        }
    }
    #[test]
    fn r3b_zero_subset_can_render_an_explicit_overall_remainder() {
        let chart=json!({"type":"pie","title":"zero","labels":["A"],"series":[{"name":"count","values":[0]}],"shareBasis":{"scope":"overall","total":100}});
        let output=render_chart_output(&chart).unwrap();
        assert_eq!(output.data_contract["charts"][0]["renderedSeries"][0]["values"],json!([0.0,100.0]));
        for value in [0.0,5e-11] {
            let small=json!({"type":"pie","title":"small","labels":["A"],"series":[{"name":"count","values":[value]}],"shareBasis":{"scope":"overall","total":1e-10}});
            let output=render_chart_output(&small).unwrap();
            assert_eq!(output.data_contract["charts"][0]["renderedLabels"],json!(["A","其他"]));
            assert!(output.data_contract["charts"][0]["renderedTotal"].as_f64().unwrap()>0.0);
            let svg=String::from_utf8(output.bytes).unwrap();assert!(!svg.contains("NaN"));
            if value>0.0 {assert!(svg.contains("50.0%"));}
        }
        let exceeds=json!({"type":"pie","title":"small","labels":["A"],"series":[{"name":"count","values":[1.5e-10]}],"shareBasis":{"scope":"overall","total":1e-10}});
        assert!(render_chart(&exceeds).is_err());
    }
    #[test]
    fn system_font_embedding_refuses_unknown_restricted_preview_bitmap_and_no_subset() {
        use ttf_parser::Permissions::*;
        for permission in [None,Some(Restricted),Some(PreviewAndPrint)] {assert!(!embedding_allowed(permission,true,true));}
        for permission in [Some(Editable),Some(Installable)] {
            assert!(embedding_allowed(permission,true,true));
            assert!(!embedding_allowed(permission,false,true));
            assert!(!embedding_allowed(permission,true,false));
        }
    }
    #[test]
    fn chart_contract_rejects_bad_shapes_and_escapes_text() {
        let bad =
            json!({"type":"pie","title":"t","labels":["a"],"series":[{"name":"s","values":[-1]}]});
        assert!(render_chart(&bad).is_err());
        let chart = json!({"type":"bar","title":"<script>&","labels":["x"],"series":[{"name":"s","values":[12]}]});
        let svg = String::from_utf8(render_chart(&chart).unwrap()).unwrap();
        assert!(svg.contains("&lt;script&gt;&amp;"));
        assert!(!svg.contains("<script>"));
        let bad = json!({"type":"line","title":"t","labels":["a","b"],"series":[{"name":"s","values":[1]}]});
        assert!(render_chart(&bad).is_err());
        for values in [json!([0]), json!([null]), json!([-2])] {
            let bad = json!({"type":"pie","title":"t","labels":["a"],"series":[{"name":"s","values":values}]});
            assert!(render_chart(&bad).is_err());
        }
        let negative = json!({"type":"bar","title":"正负变化","labels":["负值","正值"],"series":[{"name":"数值","values":[-2,3]}]});
        assert!(render_chart(&negative).is_ok());
        let excessive = json!({"type":"line","title":"t","labels":vec!["x";257],"series":[{"name":"s","values":vec![1;257]}]});
        assert!(render_chart(&excessive).is_err());
    }
    #[test]
    fn pdf_contract_checks_shape_before_font_and_obeys_stop() {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        assert!(render_pdf(
            &json!({"title":"a","blocks":[],"fontPath":"secret"}),
            &|| false,
            deadline
        )
        .is_err());
        assert!(render_pdf(
            &json!({"title":"a","blocks":[{"type":"paragraph","text":"b"}]}),
            &|| true,
            deadline
        )
        .err()
        .unwrap()
        .contains("cancelled"));
        assert!(
            render_pdf(&json!({"title":"a","blocks":[]}), &|| false, Instant::now())
                .err()
                .unwrap()
                .contains("timed_out")
        );
    }
    #[test]
    #[cfg(windows)]
    fn synthetic_vector_cjk_three_charts_and_pagination() {
        let labels = json!(["一日", "二日", "三日"]);
        let series = json!([{"name":"合成数值","values":[10,20,30]}]);
        let charts=["line","bar","pie"].map(|kind|json!({"type":"chart","chart":{"type":kind,"title":format!("合成技术验证 {kind}"),"labels":labels,"series":series}}));
        let value = json!({"title":"合成技术样例（不是 O14 业务交付）","blocks":[
            {"type":"paragraph","text":"中文文本层与矢量图表。数据总额为 60，目标增长 10%。本样例不读取业务材料。"},
            {"type":"table","columns":["指标","数值"],"rows":[["总额",60],["数量",3]]},charts[0],charts[1],charts[2],
            {"type":"paragraph","text":"分页与独立解析验证。"}]});
        let output = render_pdf(
            &value,
            &|| false,
            Instant::now() + std::time::Duration::from_secs(30),
        )
        .unwrap();
        assert!(output.bytes.starts_with(b"%PDF-"));
        assert!(output.pages >= 2);
        assert!(output.bytes.len() < 1024 * 1024);
        if let Some(dir) = std::env::var_os("FOX_PDF_TECH_SAMPLE_DIR") {
            let dir = PathBuf::from(dir);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("synthetic_vector_cjk.pdf"), &output.bytes).unwrap();
            fs::write(
                dir.join("synthetic_spec.json"),
                serde_json::to_vec_pretty(&value).unwrap(),
            )
            .unwrap();
            fs::write(dir.join("font_identity.json"),serde_json::to_vec_pretty(&json!({"font":output.font_name,"fontSha256":output.font_sha256,"pages":output.pages,"bytes":output.bytes.len(),"renderer":"krilla-0.8.2"})).unwrap()).unwrap();
        }
    }
    #[test]
    #[cfg(windows)]
    fn report_rejects_limits_and_missing_glyph_and_wraps_long_cjk() {
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        let value = json!({"title":"分页检查","blocks":[{"type":"paragraph","text":"长中文分页与换行验证。".repeat(500)}]});
        let output = render_pdf(&value, &|| false, deadline).unwrap();
        assert!(output.pages > 1);
        let oversized =
            json!({"title":"t","blocks":[{"type":"paragraph","text":"中".repeat(50000)}]});
        assert!(render_pdf(&oversized, &|| false, deadline).is_err());
        let pages = json!({"title":"t","blocks":(0..25).map(|_|json!({"type":"pageBreak"})).collect::<Vec<_>>()});
        assert!(render_pdf(&pages, &|| false, deadline).is_err());
        let chart = json!({"type":"bar","title":"这是用于验证很长的中文标题自动换行而且不覆盖坐标轴的合成图表标题这是用于技术验证","labels":["中文标签".repeat(6)],"series":[{"name":"数值","values":[-3]}]});
        assert!(render_chart(&chart).is_ok());
        let unsupported = json!({"title":"t","blocks":[{"type":"paragraph","text":"\u{10ffff}"}]});
        assert!(render_pdf(&unsupported, &|| false, deadline)
            .err()
            .unwrap()
            .contains("missing_glyph"));
    }
}
