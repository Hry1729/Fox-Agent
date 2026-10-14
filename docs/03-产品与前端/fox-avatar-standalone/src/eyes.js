/* Fox port — L3: eye renderer. Grok 25-shape morph system, recolored to
   black fill + white highlight dot. Blink/wink semantics copied from source en/ks/mn. */
(function (g) {
  const { clamp, lerpPoly, polyPath, centroid } = g.GROK_MATH;
  /* 平滑阶梯（Hermite）：与 fox.js 同式 —— 用来把"笑眼衔接"的四段曲线错开（见 EYE_ARC） */
  const smoothstep = (x, a, b) => {
    const t = clamp((x - a) / (b - a), 0, 1);
    return t * t * (3 - 2 * t);
  };
  const T = g.FOX_TABLES;
  const P = T.PLACEMENT;
  const RAW = g.FOX_EYES;

  /* —— 转正：PCA 求多边形长轴方向，吸附到最近的水平/竖直基轴。
        竖椭圆眼转成正立椭圆（歪斜消除），横扁弧线（眯眼）保持水平 —— */
  function upright(pts) {
    let sxx = 0, syy = 0, sxy = 0;
    for (const p of pts) {
      sxx += p[0] * p[0];
      syy += p[1] * p[1];
      sxy += p[0] * p[1];
    }
    const phi = 0.5 * Math.atan2(2 * sxy, sxx - syy); // 长轴相对 +x 的方向角
    const cands = [-phi, Math.PI / 2 - phi, -Math.PI / 2 - phi, Math.PI - phi];
    const a = cands.reduce((best, c) => (Math.abs(c) < Math.abs(best) ? c : best), cands[0]);
    const c = Math.cos(a), s = Math.sin(a);
    return pts.map((p) => [p[0] * c - p[1] * s, p[0] * s + p[1] * c]);
  }

  /* —— 椭圆化：向与当前 bbox 内切的标准椭圆插值。竖直眼形（含近方）抹掉墨滴棱角，
        弧线形（宽高比悬殊的横扁形）不参与，保持眯眼弧线 —— */
  function ellipsify(pts, k) {
    const xs = pts.map((p) => p[0]);
    const ys = pts.map((p) => p[1]);
    const a = Math.max((Math.max(...xs) - Math.min(...xs)) / 2, 1e-3);
    const b = Math.max((Math.max(...ys) - Math.min(...ys)) / 2, 1e-3);
    return pts.map(([x, y]) => {
      const phi = Math.atan2(y, x);
      const t = 1 / Math.hypot(Math.cos(phi) / a, Math.sin(phi) / b);
      return [
        x + (Math.cos(phi) * t - x) * k,
        y + (Math.sin(phi) * t - y) * k,
      ];
    });
  }

  /* —— 归一化：去质心 → 转正 → 缩放+纵向压缩 → 加宽/椭圆化（竖直眼形）。
        高光位置按各眼形最终几何动态计算：眼内上部 30% 高度处，略偏鼻侧 —— */
  function buildNorm() {
    const centered = RAW.map((pair) =>
      pair.map((poly) => {
        const c = centroid(poly);
        return poly.map((p) => [p[0] - c[0], p[1] - c[1]]);
      })
    );
    const up = centered.map((pair) => pair.map(upright));
    const maxR0 = Math.max(...up[0][0].map((p) => Math.hypot(p[0], p[1])));
    const k0 = P.eyeR / maxR0;
    const pipe = (pts) => {
      const mr = Math.max(...pts.map((p) => Math.hypot(p[0], p[1])));
      const k = Math.min(k0, P.maxExt / mr);
      let out = pts.map((p) => [p[0] * k, p[1] * k * P.sqY]);
      const xs = out.map((p) => p[0]);
      const ys = out.map((p) => p[1]);
      const ex = Math.max(...xs) - Math.min(...xs);
      const ey = Math.max(...ys) - Math.min(...ys);
      const vertical = ex / ey < 1.8; // 只排除横扁眯眼弧线（宽高比 ≈4+）；细竖条也要加宽椭圆化
      if (vertical) {
        if (ex / ey < P.minRatio) {
          const f = P.minRatio / (ex / ey);
          out = out.map((p) => [p[0] * f, p[1]]);
        }
        out = ellipsify(out, P.ellipseK);
      }
      return out;
    };
    /* 尺寸钳制：任何眼形最终不得超过 idle 眼的 maxGrow 倍（抑制"双眼放大变圆"）。
       cap 取自 shape0 poly0 的最终 maxR，shape0 先处理 */
    let cap = Infinity;
    const norm = up.map((pair, si) => {
      const processed = pair.map(pipe);
      if (si === 0) {
        cap =
          Math.max(...processed[0].map((p) => Math.hypot(p[0], p[1]))) *
          P.maxGrow;
      }
      return processed.map((pts) => {
        const mr = Math.max(...pts.map((p) => Math.hypot(p[0], p[1])));
        if (mr > cap) {
          const f = cap / mr;
          return pts.map((p) => [p[0] * f, p[1] * f]);
        }
        return pts;
      });
    });
    /* 每只眼睛自己的纵向范围（高光定位用）+ 每形纵向范围（闭眼检测用，沿用 poly0） */
    const extPair = norm.map((pair) =>
      pair.map((pts) => {
        const ys = pts.map((p) => p[1]);
        return Math.max(...ys) - Math.min(...ys);
      })
    );
    /* 高光点：左右各一颗。位置取"眼睛上部 30% 高度处、略偏鼻侧"，基准是
       shape0 / shape1 的纵向范围（≈ 常规睁眼高度）—— 于是高光在任何眼形里
       都稳稳待在上部，不会随眼形忽高忽低。r 来自实测。
       （原先这里写成 map 遍历全部 25 个眼形，i≥2 时 extPair[i][i] 越界成 NaN，
       只是恰好只有 0/1 会被 paintEyes 取用，才没露出来。） */
    const hl = [0, 1].map((i) => ({
      x: i === 0 ? 2 : -2,
      y: -0.3 * extPair[i][i],
      r: P.hlR[i],
    }));
    return { norm, extY: extPair.map((e) => e[0]), hl };
  }
  const NORM = buildNorm();

  /* —— 爱心眼形（"眼睛直接长成一颗心"）——
     这里刻意**不用**另挂一条 path 的做法，而是造一个 48 点的爱心多边形：
     眼形多边形恰好都是 48 点，于是「椭圆 → 爱心」可以直接逐点 lerp ——
     是一次真正的形变，而不是两个深色形状交叉溶解（后者在中途会糊成一团）。
     采样自经典心形参数方程，再等比缩放到眼形盒量级、按 bbox 居中。 —— */
  const HEART_EYE = (() => {
    const N = RAW[0][0].length;      // 48：必须与眼形点数一致
    const raw = [];
    for (let i = 0; i < N; i++) {
      const t = (i / N) * Math.PI * 2;
      const x = 16 * Math.sin(t) ** 3;
      const y =
        13 * Math.cos(t) - 5 * Math.cos(2 * t) - 2 * Math.cos(3 * t) -
        Math.cos(4 * t);
      raw.push([x, -y]);             // 翻 y：屏幕坐标下"凹口朝上、尖朝下"
    }
    const xs = raw.map((p) => p[0]);
    const ys = raw.map((p) => p[1]);
    const x0 = Math.min(...xs), x1 = Math.max(...xs);
    const y0 = Math.min(...ys), y1 = Math.max(...ys);
    const k = 90 / (x1 - x0);        // 宽度对齐 90（眼形盒宽 ≈87），等比
    const pts = raw.map((p) => [
      (p[0] - (x0 + x1) / 2) * k,
      (p[1] - (y0 + y1) / 2) * k,
    ]);
    /* 点序对齐：绕向与眼形一致、起点角度对准眼形首点 ——
       否则 lerp 中途整颗心会绕着转半圈（对不上点的对应关系）。 */
    const eye = NORM.norm[0][0];
    const area = (q) => {
      let s = 0;
      for (let i = 0; i < q.length; i++) {
        const a = q[i], b = q[(i + 1) % q.length];
        s += a[0] * b[1] - b[0] * a[1];
      }
      return s;
    };
    let h = pts;
    if (Math.sign(area(h)) !== Math.sign(area(eye))) {
      h = [h[0], ...h.slice(1).reverse()];
    }
    const ang = (p) => Math.atan2(p[1], p[0]);
    const a0 = ang(eye[0]);
    let best = 0, bd = Infinity;
    for (let i = 0; i < h.length; i++) {
      let d = Math.abs(ang(h[i]) - a0);
      d = Math.min(d, Math.PI * 2 - d);
      if (d < bd) { bd = d; best = i; }
    }
    return [...h.slice(best), ...h.slice(0, best)];
  })();

  /* —— 眨眼队列（源 queueBlink 原样，含 14% 双眨） —— */
  function queueBlink(q, now) {
    q.push(
      { at: now, v: 0.05 },
      { at: now + 70, v: 0.05 },
      { at: now + 150, v: 1.08 },
      { at: now + 300, v: 1 }
    );
    if (Math.random() < 0.14) {
      q.push({ at: now + 370, v: 0.05 }, { at: now + 480, v: 1 });
    }
  }

  function consumeBlink(q, now) {
    let key = null;
    while (q.length && now >= q[0].at) key = q.shift().v;
    return key;
  }

  /* —— wink（源 winkLid 原样） —— */
  function winkLid(base, now, winkAt, winkEye, i) {
    let lid = Math.max(base, 0.04);
    if (i === winkEye && now < winkAt + 320) {
      const xr = (now - winkAt) / 320;
      const Fr = xr < 0.42 ? 1 - xr / 0.42 : (xr - 0.42) / 0.58;
      lid = Math.max(lid * clamp(Fr, 0, 1), 0.04);
    }
    return lid;
  }

  /* —— 当前帧的两只眼形。opt.symMix：右眼向右眼形状与左眼之间的插值
        （1 = 直接用左眼形状，两眼严格同形等大；源数据里 3/5/9/12/14/18/21/23
        号眼形本身就是"一大一小"的配对，需要这个开关来抹平）。
        opt.heart：把两只眼再形变到爱心（1 = 完全变成心形）。 —— */
  function livePolys(fromIdx, toIdx, t, fromPolys, opt) {
    const from = fromPolys || NORM.norm[fromIdx];
    const to = NORM.norm[toIdx];
    const L = lerpPoly(from[0], to[0], t);
    let R = lerpPoly(from[1], to[1], t);
    const symMix = opt ? clamp(opt.symMix || 0, 0, 1) : 0;
    if (symMix > 0.001) R = lerpPoly(R, L, symMix);
    const h = opt ? clamp(opt.heart || 0, 0, 1) : 0;
    if (h > 0.001) {
      /* 心形左右对称，两只眼用同一条曲线 */
      return [lerpPoly(L, HEART_EYE, h), lerpPoly(R, HEART_EYE, h)];
    }
    return [L, R];
  }

  /* —— 眼内装饰图形（glyph） ——
     坐标系 = 该眼的局部系（原点 = 眼中心），单位与眼形一致：眼睛半高 ≈56、半宽 ≈43。
     形状都按"填满眼内空间"设计，颜色取亮色 —— 因为装饰只画在深色眼形之上，
     亮色才看得见（泪滴是唯一画到眼外的，落在白毛上，所以用蓝色）。 —— */
  /* 可用的眼内装饰图形。当前情绪表实际用到的：spark（兴奋/庆祝）、star（贪玩）、
     tear（难过）；swirl（螺旋晕圈）只在"眩晕转"特技期间强制叠加。
     ring（缩瞳环）/ heart（爱心）/ cross（叉）暂无情绪在用，保留备用。
     注：「爱心」现在有两种玩法 —— ① 眼形本体变形成心（EYE_FX 的 shape:"heart"，
     当前无人使用）；② 高光白点变白心（EYE_FX 的 hlHeart，害羞在用，见 paintEyes）。 */
  const GLYPH_KINDS = ["swirl", "ring", "spark", "star", "heart", "tear", "cross"];

  const spiralD = (turns, r0, r1, sq, steps = 96) => {
    let d = "";
    for (let k = 0; k <= steps; k++) {
      const f = k / steps;
      const th = f * turns * Math.PI * 2;
      const r = r0 + (r1 - r0) * f;
      d += (k === 0 ? "M" : "L") + (Math.cos(th) * r).toFixed(2) + " " +
        (Math.sin(th) * r * sq).toFixed(2);
    }
    return d;
  };
  /* 五角星（外径 1，内径 0.44，尖朝上）：与 fx.js 的 STAR 同一套算法 */
  const starD = (R, inner = 0.44) => {
    const n = [];
    for (let e = 0; e < 10; e++) {
      const t = -Math.PI / 2 + (e * Math.PI) / 5;
      const s = e % 2 === 0 ? R : R * inner;
      n.push(`${(Math.cos(t) * s).toFixed(2)} ${(Math.sin(t) * s).toFixed(2)}`);
    }
    return "M" + n.join("L") + "Z";
  };
  /* 四角闪光（凹边菱形）：比五角星更"闪"，用在兴奋/庆祝 */
  const sparkD = (R) =>
    `M0 ${-R}L${(R * 0.2).toFixed(2)} ${(-R * 0.24).toFixed(2)}L${R} 0` +
    `L${(R * 0.2).toFixed(2)} ${(R * 0.24).toFixed(2)}L0 ${R}` +
    `L${(-R * 0.2).toFixed(2)} ${(R * 0.24).toFixed(2)}L${-R} 0` +
    `L${(-R * 0.2).toFixed(2)} ${(-R * 0.24).toFixed(2)}Z`;
  /* 爱心：两段三次贝塞尔拼出的经典心形。实测 bbox 51.03×47.09，几何中心
     (0, −11.545) —— 后面按这个中心做"摆到目标点"的换算。 */
  const HEART_D =
    "M0 12C-19 -5 -31 -17 -23 -29C-16 -39 -3 -36 0 -25" +
    "C3 -36 16 -39 23 -29C31 -17 19 -5 0 12Z";
  const HEART_H = 47.09;
  const HEART_CY = -11.545;
  /* 爱心尺寸的参照眼高 = 标准竖椭圆眼（87.2×111.5）的高度。
     爱心高 = HEART_REF_EYE_H × EYE_HL_HEART.size × (整体缩放 s × 眼皮 lidK)，
     **与"当前用的是哪一种眼形"无关** —— 这样把害羞的眼睛从小眼换回正常大眼时，
     爱心的像素尺寸保持不变（只跟着呼吸/眨眼/眼部缩放走）。 */
  const HEART_REF_EYE_H = 111.5;
  /* 笑眼弧线（^ ^）：向上弯的粗描边，端点在眼形左右外沿、顶点在眼上沿附近 */
  const ARC_D = {
    smile: "M-46 6Q0 -40 46 6",
    sadArc: "M-46 -6Q0 40 46 -6",
  };

  /* 每帧绘制。polys 为狐狸单位（已归一化+缩放）的插值多边形。
     lids: [左,右] 眼皮系数; boost: eyeScale 弹簧值; hlGx/hlGy: 高光额外视线（局部系）
     perEyeX: [左,右] 每眼额外水平偏移（斗鸡眼用）
     arc: { mix, type } 弧线眼接管（笑眼）；glyph: { a, b, mix, phase } 眼内装饰 */
  function paintEyes(opt) {
    const {
      els, hls, hlHearts = null, decos, polys, morphT, fromIdx, toIdx, boost, lids,
      hlGx, hlGy, perEyeX = [0, 0], arc = null, glyph = null, heart = 0,
      orbit = 0, hlHeart = 0, phase = 0,
    } = opt;
    const pulse = 1 + 0.07 * Math.sin(morphT * Math.PI);
    const liveExtY =
      NORM.extY[fromIdx] + (NORM.extY[toIdx] - NORM.extY[fromIdx]) * morphT;
    const open = clamp((liveExtY - P.closedTh) / 10, 0, 1);
    const s = Math.max(boost * pulse, 0.02);
    const am = arc ? clamp(arc.mix, 0, 1) : 0;
    /* —— 笑眼衔接："先变扁、再变弯"（见 tables.EYE_ARC）——
       四段曲线的自变量都是 arcMix，靠错开的取值区间定序；反向离开时自动逆序。
         flatK   压扁进度 → bodyK：眼形纵向收缩到 squash
         arcK    弧线高度从 squash 长回 1（"那条线往上弯起来了"）
         arcVis  弧线淡入
         bodyVis 压扁的眼形本体让位（最晚） */
    const EA = T.EYE_ARC;
    const flatK = smoothstep(am, ...EA.flatT);
    const bodyK = 1 - flatK * (1 - EA.squash);
    const arcK = EA.squash + (1 - EA.squash) * smoothstep(am, ...EA.riseT);
    const arcVis = smoothstep(am, ...EA.arcT);
    const bodyVis = 1 - smoothstep(am, ...EA.bodyT);
    const hm = clamp(heart || 0, 0, 1);   // 爱心眼形接管度
    const gm = glyph ? clamp(glyph.mix, 0, 1) : 0;
    /* 装饰与弧线都会顶替高光：它们出现时高光淡出，避免"星星上叠一个白圆点"。
       关键：必须用装饰的**实际可见度**，不能直接用 glyph.mix ——
       mix 在"当前无装饰"时也停在 1（那是 A/B 溶解的完成标志），
       早期直接拿它算 → 所有状态的高光都被无条件抹掉（默认态也没了白点）。
       实际可见度：a 走 gm 淡入、b 走 1−gm 淡出，两者都空时必须是 0。 */
    const gVis = glyph && (glyph.a || glyph.b)
      ? Math.max(glyph.a ? gm : 0, glyph.b ? 1 - gm : 0)
      : 0;
    const hlCut = 1 - Math.max(am, hm, gVis);
    const orb = clamp(orbit, 0, 1);    // 高光点绕圈（困惑）
    const hh = clamp(hlHeart, 0, 1);   // 高光点变白心（害羞）

    for (let i = 0; i < 2; i++) {
      const lid = lids[i];
      const el = els[i];
      const off = perEyeX[i];
      el.setAttribute("d", polyPath(polys[i]));
      const lidK = lid * bodyK;   // 眼皮 × 笑眼衔接的"压扁"
      const tf = `translate(${off.toFixed(2)} 0) scale(${s.toFixed(4)} ${(s * lidK).toFixed(4)})`;
      el.setAttribute("transform", tf);
      /* 弧线眼：眼形本体淡出、粗弧线淡入 —— 两者都挂在同一 scale 里，随眨眼压缩 */
      if (decos) {
        const a = decos[i].arc;
        if (a) {
          a.style.display = arcVis > 0.004 ? "" : "none";
          if (arcVis > 0.004) {
            a.setAttribute("d", ARC_D[arc.type] || ARC_D.smile);
            /* 弧线用"未压扁"的 scale —— 高度从 squash 长回 1，
               观感就是"刚才压扁的那条线，现在往上弯起来了" */
            a.setAttribute(
              "transform",
              `translate(${off.toFixed(2)} 0) scale(${s.toFixed(4)} ${(s * lid * arcK).toFixed(4)})`
            );
            a.style.opacity = arcVis.toFixed(3);
          }
          const opBody = bodyVis.toFixed(3);
          if (el._opBody !== opBody) {
            el._opBody = opBody;
            el.style.opacity = opBody;
          }
        }
      }

      const hl = NORM.hl[i];
      const h = hls[i];
      /* 高光也要吃"压扁"（lidK）—— 被压扁的眼形里不该留一颗满尺寸的圆白点；
         vis 再乘 bodyVis（弧线接管后白点必须已经退场）× (1−flatK)（压扁的同时白点先淡掉，
         否则会在"那条线"中间留一颗白点 —— 只看 0.1s，但很脏）。
         早期用 am 当可见度时，压扁阶段白点还亮着，看起来像"白点没跟着眼睛一起扁"。 */
      const vis = open * clamp((lid - 0.25) / 0.45, 0, 1) * hlCut * bodyVis * (1 - flatK);
      const hr = hl.r * s * (0.8 + 0.2 * lidK);
      let hx = hl.x * s + hlGx + off;
      let hy = hl.y * lidK * s + hlGy;
      /* 绕圈（困惑）：白点沿眼形内的椭圆轨道匀速打转，两眼相位差半圈
         —— 一个在上时另一个在下，两个点各转各的才读得出"晕"，
         同步转更像"一起眨眼"。轨道半轴按当前眼形 bbox 算，随 morph 自适应，
         且乘 lid 让眨眼时轨道也跟着压扁。 */
      if (orb > 0.002) {
        const xs = polys[i].map((p) => p[0]);
        const ys = polys[i].map((p) => p[1]);
        const hw = (Math.max(...xs) - Math.min(...xs)) / 2;
        const hv = (Math.max(...ys) - Math.min(...ys)) / 2;
        const ang = phase * T.EYE_ORBIT.f * Math.PI * 2 + i * Math.PI;
        const ox = Math.cos(ang) * hw * T.EYE_ORBIT.rx * s;
        const oy = Math.sin(ang) * hv * T.EYE_ORBIT.ry * s * lidK;
        hx = hx * (1 - orb) + (off + ox) * orb;
        hy = hy * (1 - orb) + oy * orb;
      }
      h.setAttribute("cx", hx.toFixed(2));
      h.setAttribute("cy", hy.toFixed(2));
      h.setAttribute("r", hr.toFixed(2));
      h.style.opacity = (vis * (1 - hh)).toFixed(3);
      /* 高光白心（害羞）：位置按**当前眼形本体**算（心要待在眼下这只眼睛的上部），
         尺寸按**参照眼高**算（换眼形不改爱心的像素大小，见 HEART_REF_EYE_H）。
         不复用白点坐标 —— 白点定位在眼形上部，照它放大心会顶出眼形上沿。
         HEART_H / HEART_CY 是 HEART_D 的实测 bbox（getBBox：#shy 实测
         51.03×47.09、几何中心 y = −11.545）；把几何中心缩放到目标点：
         点 p → k·p，再平移 −k·CY 就能让中心落在 yT。 */
      if (hlHearts) {
        const hp = hlHearts[i];
        const von = vis * hh;
        hp.style.display = von > 0.004 ? "" : "none";
        if (von > 0.004) {
          const xs = polys[i].map((p) => p[0]);
          const ys = polys[i].map((p) => p[1]);
          const hv = ((Math.max(...ys) - Math.min(...ys)) / 2) * s * lidK;
          const k = (HEART_REF_EYE_H * T.EYE_HL_HEART.size * s * lidK) / HEART_H;
          hp.style.opacity = von.toFixed(3);
          hp.setAttribute(
            "transform",
            `translate(${off.toFixed(2)} ${(
              hv * T.EYE_HL_HEART.cy - k * HEART_CY
            ).toFixed(2)}) scale(${k.toFixed(4)})`
          );
        }
      }

      /* 眼内装饰：容器随眼形缩放，子图形各自做旋转/脉动/下落 */
      if (decos && decos[i].wrap) {
        const wrap = decos[i].wrap;
        wrap.style.display = gVis > 0.004 ? "" : "none";
        if (gVis > 0.004) {
          wrap.setAttribute("transform", tf);
          wrap.style.opacity = gm.toFixed(3);
          paintGlyph(decos[i], glyph, gm, i);
        }
      }
    }
  }

  /* 单个眼内装饰：a 淡入 / b 淡出，并按类型施加运动 */
  function paintGlyph(deco, glyph, gm, i) {
    const { a, b, phase } = glyph;
    for (const kind of GLYPH_KINDS) {
      const slot = deco.kind[kind];
      if (!slot) continue;
      const { node, inner } = slot;
      const op = kind === a ? gm : kind === b ? 1 - gm : 0;
      node.style.opacity = op.toFixed(3);
      node.style.display = op > 0.004 ? "" : "none";
      if (op <= 0.004) continue;
      if (kind === "swirl") {
        node.setAttribute("transform", `rotate(${((phase * 96) % 360).toFixed(1)})`);
      } else if (kind === "spark" || kind === "star") {
        const k = 0.86 + 0.14 * Math.sin(phase * 6.2 + i * Math.PI);
        node.setAttribute("transform", `scale(${k.toFixed(3)})`);
      } else if (kind === "heart") {
        const k = 1 + 0.09 * Math.sin(phase * 7.4 + i * 1.7);
        node.setAttribute("transform", `scale(${k.toFixed(3)})`);
      } else if (kind === "tear") {
        /* 泪珠：从眼角下滑到眼外，末段淡出后循环 */
        const f = (phase * 0.62 + i * 0.5) % 1;
        const cy = 30 + f * 78;
        inner.setAttribute("cy", cy.toFixed(1));
        node.style.opacity = (op * (1 - Math.pow(Math.max(f - 0.55, 0) / 0.45, 1.5))).toFixed(3);
      } else if (kind === "ring") {
        const k = 1 + 0.06 * Math.sin(phase * 5.1);
        node.setAttribute("transform", `scale(${k.toFixed(3)})`);
      }
    }
  }

  function buildEls(svg) {
    const ns = "http://www.w3.org/2000/svg";
    const gazeG = document.createElementNS(ns, "g");
    const faceG = document.createElementNS(ns, "g");
    faceG.setAttribute(
      "transform",
      `translate(${P.mid[0]} ${P.mid[1]}) rotate(${P.theta})`
    );

    /* —— 鼻子：底图实测的椭圆（见 PLACEMENT.nose），换算到脸局部系后挂在 faceG 的
          兄弟组里（不是 faceG 内！）。原因：faceG 在 gazeG 里会跟着视线平移，而底图
          上的鼻子是不动的 —— 挂进去会让鼻子比底图鼻子多漂 0.5px，特写时叠出双影。
          组变换与 faceG 逐帧同参（translate(mid) rotate(theta)），于是"与眼睛同一
          坐标系、同一比例"，只差一个视线偏移。
          只在皮肤声明 faceNose 时才建（index-eyes.html：脸被纯色盖板压平、底图鼻子
          被盖掉，需要补一个）。主页面不建 —— 底图自带鼻子，而底图鼻子不是完美椭圆
          （实测径向 26.2~45.8px），任何矢量副本要么越界留一圈 1px 接缝、要么内切
          缩水 11%，两头都不如"什么都不画"。
          先于 gazeG append → 眼睛仍在最上层。平时 display:none，特写态由 fox.js 淡入。 —— */
    const SKIN = window.FOX_SKIN || {};
    let noseG = null;
    if (P.nose && SKIN.faceNose) {
      const th = (P.theta * Math.PI) / 180;
      const dx = P.nose.at[0] - P.mid[0];
      const dy = P.nose.at[1] - P.mid[1];
      const u = dx * Math.cos(th) + dy * Math.sin(th); // R(−θ)·d：把绝对坐标换成脸局部
      const v = -dx * Math.sin(th) + dy * Math.cos(th);
      noseG = document.createElementNS(ns, "g");
      noseG.setAttribute(
        "transform",
        `translate(${P.mid[0]} ${P.mid[1]}) rotate(${P.theta})`
      );
      const ell = document.createElementNS(ns, "ellipse");
      ell.setAttribute("rx", P.nose.rx.toFixed(2));
      ell.setAttribute("ry", P.nose.ry.toFixed(2));
      ell.setAttribute("fill", P.noseFill || P.eyeFill);
      ell.setAttribute(
        "transform",
        `translate(${u.toFixed(2)} ${v.toFixed(2)}) rotate(${(P.nose.rot - P.theta).toFixed(2)})`
      );
      noseG.appendChild(ell);
      noseG.setAttribute("pointer-events", "none");
      noseG.style.opacity = "0";
      noseG.style.display = "none";
      svg.appendChild(noseG);
    }

    const els = [];
    const hls = [];
    const hlHearts = [];
    const decos = [];
    for (let i = 0; i < 2; i++) {
      const eyeG = document.createElementNS(ns, "g");
      eyeG.setAttribute(
        "transform",
        `translate(${(i === 0 ? -P.eyeDX : P.eyeDX).toFixed(2)} 0)`
      );
      const path = document.createElementNS(ns, "path");
      path.setAttribute("fill", P.eyeFill);
      const hl = document.createElementNS(ns, "circle");
      hl.setAttribute("fill", P.hlFill);
      /* 高光白点也可以是一颗白心（害羞）：与圆点同位同心，两者交叉淡入淡出
         （圆的 d 与心的 d 都是白色，重叠时读作"白点长成一颗心"）。
         HEART_D 的几何中心在 (0, −13.5)，所以 transform 里先 translate(0 13.5)
         把心摆正到原点，再缩放、再平移到高光位置。 */
      const hlHeart = document.createElementNS(ns, "path");
      hlHeart.setAttribute("d", HEART_D);
      hlHeart.setAttribute("fill", P.hlFill);
      hlHeart.style.display = "none";
      hlHeart.style.opacity = "0";
      /* 弧线眼（笑眼）：粗圆头描边，端点在眼形左右外沿、顶点接近眼上沿 */
      const arc = document.createElementNS(ns, "path");
      arc.setAttribute("fill", "none");
      arc.setAttribute("stroke", P.eyeFill);
      arc.setAttribute("stroke-width", "15");
      arc.setAttribute("stroke-linecap", "round");
      arc.style.display = "none";
      arc.style.opacity = "0";
      /* 眼内装饰容器：整体随眼形 scale 缩放（于是眨眼时装饰同步压缩） */
      const wrap = document.createElementNS(ns, "g");
      wrap.setAttribute("pointer-events", "none");
      const kind = {};
      for (const k of GLYPH_KINDS) {
        const node = document.createElementNS(ns, "g");
        node.setAttribute("pointer-events", "none");
        node.style.display = "none";
        node.style.opacity = "0";
        const inner = document.createElementNS(ns, "g");
        /* 各图形的几何：全部按"填满眼内"设计（眼半高 56 / 半宽 43） */
        if (k === "swirl") {
          const p = document.createElementNS(ns, "path");
          p.setAttribute("d", spiralD(2.2, 5, 38, 0.72));
          p.setAttribute("fill", "none");
          p.setAttribute("stroke", P.hlFill);
          p.setAttribute("stroke-width", "5.5");
          p.setAttribute("stroke-linecap", "round");
          inner.appendChild(p);
        } else if (k === "ring") {
          const c1 = document.createElementNS(ns, "circle");
          c1.setAttribute("r", "29");
          c1.setAttribute("fill", "none");
          c1.setAttribute("stroke", P.hlFill);
          c1.setAttribute("stroke-width", "6");
          const c2 = document.createElementNS(ns, "circle");
          c2.setAttribute("r", "8.5");
          c2.setAttribute("fill", P.hlFill);
          inner.appendChild(c1);
          inner.appendChild(c2);
        } else if (k === "spark") {
          const p = document.createElementNS(ns, "path");
          p.setAttribute("d", sparkD(36));
          p.setAttribute("fill", P.hlFill);
          inner.appendChild(p);
        } else if (k === "star") {
          const p = document.createElementNS(ns, "path");
          p.setAttribute("d", starD(32));
          p.setAttribute("fill", P.hlFill);
          inner.appendChild(p);
        } else if (k === "heart") {
          const p = document.createElementNS(ns, "path");
          p.setAttribute("d", HEART_D);
          p.setAttribute("transform", "translate(0 8)");
          p.setAttribute("fill", "#ff7a9a");
          inner.appendChild(p);
        } else if (k === "tear") {
          const e = document.createElementNS(ns, "ellipse");
          e.setAttribute("rx", "11");
          e.setAttribute("ry", "15");
          e.setAttribute("cy", "30");
          e.setAttribute("fill", "#6fb3ff");
          inner.appendChild(e);
        } else if (k === "cross") {
          for (const d of ["M-25 -25L25 25", "M-25 25L25 -25"]) {
            const l = document.createElementNS(ns, "path");
            l.setAttribute("d", d);
            l.setAttribute("stroke", P.hlFill);
            l.setAttribute("stroke-width", "9");
            l.setAttribute("stroke-linecap", "round");
            l.setAttribute("fill", "none");
            inner.appendChild(l);
          }
        }
        node.appendChild(inner);
        wrap.appendChild(node);
        kind[k] = { node, inner };
      }
      wrap.style.display = "none";
      wrap.style.opacity = "0";
      eyeG.appendChild(path);
      eyeG.appendChild(hl);
      eyeG.appendChild(hlHeart);
      eyeG.appendChild(arc);
      eyeG.appendChild(wrap);
      faceG.appendChild(eyeG);
      els.push(path);
      hls.push(hl);
      hlHearts.push(hlHeart);
      decos.push({ arc, wrap, kind });
    }
    /* 眉毛（曾挂在 browG 里，与鼻子同套路）已按需求整体移除：
       见 tables.js 的说明 —— 这里只有眼睛和鼻子，两道弧线不属于这张脸。 */
    gazeG.appendChild(faceG);
    svg.appendChild(gazeG);
    return { gazeG, faceG, els, hls, hlHearts, decos, nose: noseG };
  }

  g.FOX_EYES_MOD = {
    NORM, queueBlink, consumeBlink, winkLid, livePolys, paintEyes, buildEls,
    GLYPH_KINDS,
  };
})(window);
