/* Fox port — L3: overlay glyphs + burst particles.
   Ported from Grok fx.js: all 14 overlay paints + extras + simple burst particles.
   Overlays are painted in SOURCE coordinate space (body radius R0) inside a scaled
   wrapper; fox.js composites body transforms using k = 360/R0. */
(function (g) {
  const NS = "http://www.w3.org/2000/svg";
  const R0 = 114.2705;        // 源空间体半径
  const K = 360 / R0;         // 源空间 → 狐狸 720 空间
  const STAR_COLOR = "#f4c34e";
  const PALETTE = ["#f9705c", "#5b95f0", "#3fbe86", "#f5b13f", "#9a72ee", "#35c3bd"];
  const STAR = (() => {
    const n = [];
    for (let e = 0; e < 10; e++) {
      const t = -Math.PI / 2 + (e * Math.PI) / 5;
      const s = e % 2 === 0 ? 1 : 0.42;
      n.push(`${(Math.cos(t) * s).toFixed(3)} ${(Math.sin(t) * s).toFixed(3)}`);
    }
    return "M" + n.join("L") + "Z";
  })();

  const MAP = {
    thinking: "dots", orbit: "orbit", radar: "radar", progress: "progress",
    spawning: "gather", dictating: "wave", sending: "send", receiving: "receive",
    uploading: "dock", bouncing: "ball", loading: "whirl",
    "powering-down": "standby", writing: "pencil", alerting: "bang",
  };
  const CYCLE = new Set(["progress", "spawning"]);
  const CYCLE_ON = { progress: 2500, spawning: 2000 };
  const CYCLE_OFF = 1500;
  const SCALE = {
    dots: 1.5, orbit: 1.14, radar: 1.14, progress: 1.32, gather: 1.15,
    wave: 1.42, send: 1.12, receive: 1.12, dock: 1.3, ball: 1.22,
    whirl: 1.45, pencil: 1.18, bang: 1.28, standby: 1.75,
  };
  const RADIUS = {
    dots: 22, orbit: 19, radar: 19, progress: 19, gather: 19, wave: 16,
    send: 20, receive: 20, dock: 20, ball: 18, whirl: 15, pencil: 17,
    bang: 13, standby: 13,
  };
  const P_BLEND = 0.62;
  const DOT_R = 22, DOT_GAP = 62, POP0 = 0.84, POP1 = 0.22;
  const SEND_MS = 1500, RECV_MS = 1700, PENCIL_MS = 2500;   // 发送/接收/书写每轮时长
  /* 发送·接收共用同一段"出射方向"取值域（弧度）——两者读起来才是同一套语言：
     接收从这段方向进来，发送往这段方向出去。
     弧段 -1.25π→+0.25π（= -225°→+45°）扫过左侧、上方到右侧，**避开正下方**：
     朝下的点会贴着下巴/脖子飞出去，在小尺寸缩点里根本读不出来。
     源版发送把方向写死成 (0.74, -0.62)（≈ -40°，即本弧段内的一个固定点），
     于是每 1.5s 都往同一处飞、读起来像机械臂；接收则一直是随机来向。 */
  const DIR_ARC = [-Math.PI * 1.25, Math.PI * 0.25];

  const clamp = (n, a, b) => Math.min(b, Math.max(a, n));
  const rand = (a, b) => a + Math.random() * (b - a);
  const Rc = (n) => 1 - Math.pow(1 - n, 3);
  const y1e = (n) => 1 + 2.70158 * Math.pow(n - 1, 3) + 1.70158 * Math.pow(n - 1, 2);
  const K2 = (n) => (n < 0.5 ? 4 * n * n * n : 1 - Math.pow(-2 * n + 2, 3) / 2);

  function el(tag, attrs) {
    const n = document.createElementNS(NS, tag);
    if (attrs) for (const k in attrs) n.setAttribute(k, attrs[k]);
    return n;
  }

  function closedSpline(pts) {
    const e = pts.length;
    let t = `M${pts[0][0].toFixed(2)} ${pts[0][1].toFixed(2)}`;
    for (let s = 0; s < e; s++) {
      const r = pts[(s - 1 + e) % e], i = pts[s], o = pts[(s + 1) % e], l = pts[(s + 2) % e];
      t += `C${(i[0] + (o[0] - r[0]) / 6).toFixed(2)} ${(i[1] + (o[1] - r[1]) / 6).toFixed(2)} ${(o[0] - (l[0] - i[0]) / 6).toFixed(2)} ${(o[1] - (l[1] - i[1]) / 6).toFixed(2)} ${o[0].toFixed(2)} ${o[1].toFixed(2)}`;
    }
    return t + "Z";
  }

  function circleRing(R, n = 96) {
    return Array.from({ length: n }, (_, e) => {
      const t = (e / n) * Math.PI * 2;
      return [R + Math.cos(t) * R, R + Math.sin(t) * R];
    });
  }

  function capsule(n, e, R) {
    const t = n / 2, s = R - e / 2 + t, r = R + e / 2 - t;
    return `M${R - t} ${s}A${t} ${t} 0 0 1 ${R + t} ${s}L${R + t} ${r}A${t} ${t} 0 0 1 ${R - t} ${r}Z`;
  }

  function taper(n, e, t, R) {
    const s = n / 2, r = e / 2, i = R - t / 2, o = R + t / 2;
    return `M${R - s} ${i + s}A${s} ${s} 0 0 1 ${R + s} ${i + s}L${R + r} ${o - r}A${r} ${r} 0 0 1 ${R - r} ${o - r}Z`;
  }

  function smoothLine(pts) {
    const Pt = pts.length;
    let mt = `M${pts[0][0].toFixed(1)} ${pts[0][1].toFixed(1)}`;
    if (Pt === 2) return mt + `L${pts[1][0].toFixed(1)} ${pts[1][1].toFixed(1)}`;
    for (let Dt = 0; Dt < Pt - 1; Dt++) {
      const Mt = pts[Math.max(Dt - 1, 0)], Lt = pts[Dt], yn = pts[Dt + 1], an = pts[Math.min(Dt + 2, Pt - 1)];
      mt += `C${(Lt[0] + (yn[0] - Mt[0]) / 6).toFixed(1)} ${(Lt[1] + (yn[1] - Mt[1]) / 6).toFixed(1)} ${(yn[0] - (an[0] - Lt[0]) / 6).toFixed(1)} ${(yn[1] - (an[1] - Lt[1]) / 6).toFixed(1)} ${yn[0].toFixed(1)} ${yn[1].toFixed(1)}`;
    }
    return mt;
  }

  function wave(ze) {
    return 0.42 + 0.29 * Math.sin(ze * 0.0021) * Math.sin(ze * 0.0034) + 0.29 * Math.sin(ze * 0.0013 + 1.7);
  }

  function pencilPose(now, stateAt) {
    const Pt = now - stateAt;
    const mt = (((Pt / PENCIL_MS) % 1) + 1) % 1;
    if (mt < 0.68) {
      const Mt = mt / 0.68;
      const Lt = Mt * Mt * (3 - 2 * Mt);
      const yn = clamp(Mt / 0.08, 0, 1) * clamp((1 - Mt) / 0.08, 0, 1);
      return { x: -54 + 118 * Lt, y: 26, wig: Math.sin(Mt * 24) * 3.2 * yn, rot: 17 + Math.sin(Pt * 6e-4) * 1, lift: false };
    }
    const Dt = K2((mt - 0.68) / 0.32);
    return { x: 64 - 118 * Dt, y: 26 - 20 * Math.sin(Dt * Math.PI), wig: 0, rot: 17 - 2 * Math.sin(Dt * Math.PI) + Math.sin(Pt * 6e-4) * 1, lift: true };
  }

  /* —— 撒花粒子（源 burst 裁剪版：矩形/圆/星星三种，去掉轨道丝带） ——
     palette/star 可被皮肤替换（蓝盘主题把六色彩虹换成蓝系，见 tables.ORN_BURST）。 */
  function createParticles({ back, front, palette, star }) {
    const PAL = palette && palette.length ? palette : PALETTE;
    const STAR_C = star || STAR_COLOR;
    let parts = [];
    let last = -1;
    const burst = (N = 20, H = 1, G = 0) => {
      if (!back || parts.length > 120) return;
      for (let i = 0; i < N; i++) {
        const U = (i / N) * Math.PI * 2 + rand(-0.35, 0.35);
        const ee = rand(96, 116) * K * 0.92;
        const te = rand(170, 360) * H * K;
        const ne = -Math.sin(U), j = Math.cos(U), Z = G * te * 0.2;
        const X = Math.random() < 0.18;
        parts.push({
          x: 360 + Math.cos(U) * ee, y: 360 + Math.sin(U) * ee,
          vx: Math.cos(U) * te + ne * Z, vy: Math.sin(U) * te + j * Z - rand(20, 75) * K,
          life: 0, max: rand(0.45, 0.85),
          r: (X ? rand(4, 7) : rand(3.5, 8)) * K,
          rot: rand(0, 360), vr: rand(-260, 260),
          color: X ? STAR_C : PAL[(Math.random() * PAL.length) | 0],
          round: !X && Math.random() < 0.3, star: X, el: null,
        });
      }
    };
    const step = (dt) => {
      if (!parts.length) return;
      const keep = [];
      for (const j of parts) {
        j.life += dt;
        if (j.life >= j.max) { j.el?.remove(); continue; }
        j.x += j.vx * dt; j.y += j.vy * dt;
        const se = Math.pow(0.94, dt * 60);
        j.vx *= se; j.vy = j.vy * se + 40 * K * dt;
        const le = j.life / j.max;
        const Q = le < 0.1 ? le / 0.1 : Math.pow(1 - (le - 0.1) / 0.9, 1.7);
        const ae = Math.max(j.r * (1 - le * 0.4), 0.5);
        if (!j.el) {
          const ce = el(j.star ? "path" : j.round ? "circle" : "rect");
          if (j.star) ce.setAttribute("d", STAR);
          ce.setAttribute("fill", j.color);
          back.appendChild(ce); j.el = ce;
        }
        j.el.setAttribute("opacity", Q.toFixed(3));
        if (j.star) {
          j.rot += j.vr * dt;
          j.el.setAttribute("transform", `translate(${j.x.toFixed(1)} ${j.y.toFixed(1)}) rotate(${j.rot.toFixed(1)}) scale(${ae.toFixed(2)})`);
        } else if (j.round) {
          j.el.setAttribute("cx", j.x.toFixed(1)); j.el.setAttribute("cy", j.y.toFixed(1)); j.el.setAttribute("r", ae.toFixed(2));
        } else {
          const ce = Math.hypot(j.vx, j.vy);
          const xe = Math.max(ae * 2, Math.min(ce * 0.05, 94)), Se = ae * 1.5;
          const fe = (Math.atan2(j.vy, j.vx) * 180) / Math.PI;
          j.el.setAttribute("width", xe.toFixed(1)); j.el.setAttribute("height", Se.toFixed(1));
          j.el.setAttribute("rx", (Se / 2).toFixed(2));
          j.el.setAttribute("x", (j.x - xe / 2).toFixed(1)); j.el.setAttribute("y", (j.y - Se / 2).toFixed(1));
          j.el.setAttribute("transform", `rotate(${fe.toFixed(1)} ${j.x.toFixed(1)} ${j.y.toFixed(1)})`);
        }
        keep.push(j);
      }
      parts = keep;
    };
    return {
      burst,
      clear() { for (const w of parts) w.el?.remove(); parts = []; },
      update(now, dt) {
        const Y = last < 0 ? dt : Math.max((now - last) / 1000, 0);
        last = now;
        step(Y);
      },
      hasLife: () => parts.length > 0,
    };
  }

  /* —— overlay 图层：back/前景分层，全部在源空间绘制 ——
     color = 主色，alt = 可选的第二色（蓝盘主题传"淡蓝 / 白"两色）。
     单色时所有元素同色（原语义，靠透明度分深浅）；给了 alt 就让**逐个元素交替**：
     跳动点两个一淡蓝一白、环绕粒子五颗相间、圈用主色 —— 这样才读得出"两个颜色的
     小圆"，而不是"同一个圆深浅不同"。 */
  class OverlayLayer {
    constructor(color = "var(--fg)", alt = null) {
      this.fg = color;
      const c2 = alt || color;
      this.root = el("g", { "aria-hidden": "true", transform: `scale(${(360 / R0).toFixed(5)})` });
      this.back = el("g");
      this.dots = [color, c2].map((c) => el("path", { style: `fill:${c};display:none` }));
      this.rings = [0, 1, 2, 3, 4, 5, 6].map(() =>
        el("circle", { cx: "0", cy: "0", r: "0", fill: "none", style: `display:none;stroke:${color}` })
      );
      this.parts = [0, 1, 2, 3, 4, 5, 6].map((i) =>
        el("circle", { cx: "0", cy: "0", r: "0", style: `fill:${i % 2 ? c2 : color};display:none` })
      );
      this.glyphs = [0, 1, 2].map(() => el("path", { style: "display:none" }));
      this.front = el("g");
      [this.back, ...this.dots, ...this.rings, ...this.parts, ...this.glyphs, this.front]
        .forEach((n) => this.root.appendChild(n));
      this.ink = [];
      /* 发送/接收的出射方向与"轮次"游标。方向每换一轮（SEND_MS / RECV_MS）重掷一次，
         一轮之内恒定 —— 每帧重掷会抖成噪点。初始值 -0.7 rad ≈ 源版发送的写死方向，
         仅作为第一轮之前的兜底（sendTick/recvTick = -1 保证第 0 轮就会立刻重掷）。 */
      this.recvDir = -0.7;
      this.recvTick = -1;
      this.sendDir = -0.7;
      this.sendTick = -1;
      this.overlayAt = 0;
      this._reduce = false;
      /* 周围小圆的"暗端下限"（源版语义 = 跳动点脉冲最暗 0.5、环绕粒子背面 0.18）。
         深色底上 0.18 的浅色圆直接读成"黑色小圆"；蓝盘主题把两个下限一起抬起，
         于是每颗小圆任何时刻都读得出是淡蓝/白。写回默认值即完全回到原版。 */
      this.orbitFloor = 0.18;
      this.dotFloor = 0.5;
      this.circlePath = closedSpline(circleRing(R0));
      this.pencilPath = capsule(30, 88, R0);
      this.bangPath = taper(30, 17, 96, R0);
    }

    hideAll() {
      for (const n of this.dots) n.style.display = "none";
      for (const n of this.rings) n.style.display = "none";
      for (const n of this.parts) n.style.display = "none";
      for (const n of this.glyphs) n.style.display = "none";
    }

    amount(name, cur, prev, yl, mix) {
      if (name === cur) return yl * mix;
      if (name === prev) return yl * (1 - mix);
      return 0;
    }

    dotsPulse(now, slot, yl, reduce = false) {
      const Dt = ((((now - this.overlayAt) / 1400 + 0.119) % 1) + 1) % 1;
      let Mt = Math.abs(Dt - slot / 3);
      Mt = Math.min(Mt, 1 - Mt);
      const Lt = reduce ? 1 : Math.exp(-(Mt * Mt) / (2 * 0.15 * 0.15));
      const yn = reduce ? 0 : 1;
      return { lift: Lt * 9 * yl * yn, pop: 1 + yn * (POP0 + POP1 * Lt - 1), tone: Math.max(1 - yn * 0.5 * (1 - Lt), this.dotFloor) };
    }

    paint(now, stateAt, cur, prev, yl, mix, reduce = false) {
      this.hideAll();
      this._reduce = reduce;
      const extra = this.extras(now, stateAt, cur, prev, yl, mix);
      const kl = (name) => this.amount(name, cur, prev, yl, mix);
      const run = (name, fn) => { const a = kl(name); if (a > 0.004) fn(a); };
      run("dots", (a) => this.paintDots(a, now, yl));
      run("orbit", (a) => this.paintOrbit(a, now));
      run("radar", (a) => this.paintRadar(a, now, extra.A2));
      run("progress", (a) => this.paintProgress(a, now));
      run("gather", (a) => this.paintGather(a, now));
      run("wave", (a) => this.paintWave(a, now));
      run("send", (a) => this.paintSend(a, now, stateAt));
      run("receive", (a) => this.paintRecv(a, now, stateAt));
      run("dock", (a) => this.paintDock(a, now, stateAt));
      run("pencil", (a) => this.paintPencil(a, now, stateAt));
      run("bang", (a) => this.paintBang(a, now, stateAt));
      run("standby", (a) => this.paintStandby(a));
      return extra;
    }

    paintDots(ze, now, yl) {
      const mt = [R0 - DOT_GAP, R0 + DOT_GAP];
      for (let Dt = 0; Dt < 2; Dt++) {
        const Mt = this.dots[Dt];
        const Lt = clamp((ze - Dt * 0.12) / (1 - Dt * 0.12), 0, 1);
        if (Lt <= 0.004) { Mt.style.display = "none"; continue; }
        const yn = Rc(Lt), an = y1e(Lt), Et = this.dotsPulse(now, Dt === 0 ? 0 : 2, ze, this._reduce);
        const En = (DOT_R * yn * Et.pop) / R0 * 1.02;
        Mt.style.display = "";
        Mt.setAttribute("d", this.circlePath);
        Mt.setAttribute("transform", `translate(${(R0 + (mt[Dt] - R0) * an).toFixed(1)} ${(R0 - Et.lift).toFixed(1)}) scale(${En.toFixed(4)}) translate(${-R0} ${-R0})`);
        Mt.setAttribute("opacity", (yn * Et.tone).toFixed(3));
      }
    }

    paintOrbit(ze, now) {
      const mt = Rc(ze), Mt = 52 * y1e(ze), Lt = 12, yn = now * 0.0017;
      for (let an = 0; an < 5; an++) {
        const Et = this.parts[an];
        const En = yn + (an * Math.PI * 2) / 5, Zt = Math.cos(En), dn = 0.5 + 0.5 * clamp(Zt, 0, 1);
        Et.style.display = "";
        Et.setAttribute("cx", (R0 + Mt * Math.sin(En)).toFixed(1));
        Et.setAttribute("cy", (R0 - Mt * 0.42 * Math.cos(En)).toFixed(1));
        Et.setAttribute("r", Math.max(Lt * dn * mt, 0.3).toFixed(2));
        Et.setAttribute("opacity", (clamp((Zt + 0.4) / 0.6, this.orbitFloor, 1) * mt).toFixed(3));
      }
    }

    paintRadar(ze, now, bodyR) {
      const Dt = Rc(ze);
      for (let yn = 0; yn < 3; yn++) {
        const an = this.rings[yn];
        const Et = (now / 1300 + yn / 3) % 1;
        an.style.display = "";
        an.removeAttribute("stroke-dasharray");
        an.removeAttribute("transform");
        an.setAttribute("cx", `${R0}`); an.setAttribute("cy", `${R0}`);
        an.setAttribute("r", (bodyR + (104 - bodyR) * Et).toFixed(1));
        an.setAttribute("stroke-width", (3.4 * (1 - Et * 0.55)).toFixed(2));
        an.setAttribute("opacity", (Dt * (1 - Et) * 0.9).toFixed(3));
      }
    }

    paintProgress(ze, now) {
      const mt = Rc(ze), Dt = y1e(ze), Mt = 62;
      const Lt = clamp((now - this.overlayAt) / CYCLE_ON.progress, 0, 1);
      const yn = clamp(Lt / 0.85, 0, 1);
      const an = this.rings[3];
      an.style.display = "";
      an.setAttribute("cx", `${R0}`); an.setAttribute("cy", `${R0}`);
      an.setAttribute("r", (Mt * Dt).toFixed(1));
      an.setAttribute("stroke-width", "5");
      an.removeAttribute("stroke-dasharray");
      an.removeAttribute("transform");
      an.setAttribute("opacity", (mt * 0.16).toFixed(3));
      const Et = this.rings[4];
      const En = Mt * Dt, Zt = 2 * Math.PI * En;
      Et.style.display = "";
      Et.setAttribute("cx", `${R0}`); Et.setAttribute("cy", `${R0}`);
      Et.setAttribute("r", En.toFixed(1));
      Et.setAttribute("stroke-width", "5");
      Et.setAttribute("stroke-dasharray", `${Zt.toFixed(1)}`);
      Et.setAttribute("stroke-dashoffset", (Zt * (1 - yn)).toFixed(1));
      Et.setAttribute("transform", `rotate(-90 ${R0} ${R0})`);
      Et.setAttribute("opacity", mt.toFixed(3));
    }

    paintGather(ze, now) {
      const mt = Rc(ze), Dt = CYCLE_ON.spawning;
      for (let Mt = 0; Mt < 5; Mt++) {
        const Lt = this.parts[Mt];
        const yn = clamp(((now - this.overlayAt) / Dt - Mt * 0.09) / 0.62, 0, 1);
        if (yn >= 1) { Lt.style.display = "none"; continue; }
        const an = 1 - Math.pow(1 - yn, 3), Et = Mt * 2.4 + yn * 2.2, En = 96 * (1 - an);
        Lt.style.display = "";
        Lt.setAttribute("cx", (R0 + En * Math.cos(Et)).toFixed(1));
        Lt.setAttribute("cy", (R0 + En * Math.sin(Et) * 0.8).toFixed(1));
        Lt.setAttribute("r", (9 * (0.5 + 0.5 * an) * mt).toFixed(2));
        Lt.setAttribute("opacity", (mt * clamp(yn * 5, 0, 1) * (1 - an * 0.25)).toFixed(3));
      }
    }

    /* 语音（dictating = "wave"）：脸心左右各两颗小圆，随声波一起胀缩，像一组音量条。
       圆心**钉在 R0 不动**，只有半径在变 ⇒ 缩放是各向同性的"以圆心为中心放大"。
       （源版此处另有一项竖直抬升 `dn = 6·clamp(En,0,1)·an`：半径涨 9 的同时圆心还要
        上移 6，于是下边缘几乎不动、上边缘抬 18 —— 读起来是"从底边长上来"的一根柱子，
        而不是一颗在原地胀大的圆。本轮把这个 `dn` 去掉，只保留半径变化。） */
    paintWave(ze, now) {
      const mt = [-2, -1, 1, 2], Dt = 44;
      for (let Mt = 0; Mt < 4; Mt++) {
        const Lt = Mt < 2 ? this.dots[Mt] : this.parts[3 + Mt];
        const yn = mt[Mt];
        const an = clamp((ze - Math.abs(yn) * 0.1) / (1 - Math.abs(yn) * 0.1), 0, 1);
        if (an <= 0.004) { Lt.style.display = "none"; continue; }
        const Et = y1e(an), En = wave(now) * (0.55 + 0.45 * Math.sin(now * 0.012 - Math.abs(yn) * 1.05));
        const Zt = (7 + 9 * clamp(En, 0.08, 1)) * Rc(an);
        Lt.style.display = "";
        if (Mt < 2) {
          /* 左二用 path + transform：`translate(圆心) scale(s) translate(-R0,-R0)`
             把圆路径（圆心 (R0,R0)）先挪到原点再缩放再挪到目标点 ⇒ 缩放中心 = 那个目标点
             本身，天然是圆心不动。这里 cy 直接写 R0（不再减 dn）。 */
          const on = (Zt / R0) * 1.02;
          Lt.setAttribute("d", this.circlePath);
          Lt.setAttribute("transform", `translate(${(R0 + yn * Dt * Et).toFixed(1)} ${R0}) scale(${on.toFixed(4)}) translate(${-R0} ${-R0})`);
          Lt.setAttribute("opacity", an.toFixed(3));
        } else {
          /* 右二用原生 circle：cx/cy 就是圆心，r 单独长 ⇒ 圆心同样固定。 */
          Lt.setAttribute("cx", (R0 + yn * Dt * Et).toFixed(1));
          Lt.setAttribute("cy", `${R0}`);
          Lt.setAttribute("r", Zt.toFixed(2));
          Lt.setAttribute("opacity", an.toFixed(3));
        }
      }
    }

    /* 发送（sending）：一颗点从脸心直线飞出去 + 一圈涟漪。方向**每轮（SEND_MS）随机重掷**，
       取值域与接收共用（DIR_ARC）⇒ 发送与接收从此对称：接收的来向是随机的，
       发送的去向也随机。用 `sendTick` 只在**换轮**时重掷，一轮之内方向恒定
       （每帧重掷会抖成噪点），写法与 paintRecv 的 recvTick 完全一致。
       `yn/an` 就是单位方向向量（原来是写死的常数 0.74 / -0.62，即右上 ≈ -40°）。 */
    paintSend(ze, now, stateAt) {
      const mt = Rc(ze), Dt = ((((now - stateAt) / SEND_MS) % 1) + 1) % 1;
      const tick = Math.floor((now - stateAt) / SEND_MS);
      if (tick !== this.sendTick) { this.sendTick = tick; this.sendDir = rand(DIR_ARC[0], DIR_ARC[1]); }
      const Mt = clamp((Dt - 0.18) / 0.55, 0, 1), Lt = Mt * Mt * (0.4 + 0.6 * Mt);
      const yn = Math.cos(this.sendDir), an = Math.sin(this.sendDir), Et = 108 * Lt, En = this.parts[5];
      const on = Mt > 0 && Mt < 1;
      En.style.display = on ? "" : "none";
      if (on) {
        En.setAttribute("cx", (R0 + yn * Et).toFixed(1));
        En.setAttribute("cy", (R0 + an * Et).toFixed(1));
        En.setAttribute("r", (10 * (1 - Lt * 0.55) * mt).toFixed(2));
        En.setAttribute("opacity", (mt * (1 - Lt * Lt)).toFixed(3));
      }
      const Zt = this.parts[6];
      const on2 = clamp((Dt - 0.26) / 0.55, 0, 1), bn = on2 * on2 * (0.4 + 0.6 * on2);
      const Cn = Mt > 0 && on2 > 0 && on2 < 1;
      Zt.style.display = Cn ? "" : "none";
      if (Cn) {
        const bi = 108 * bn;
        Zt.setAttribute("cx", (R0 + yn * bi).toFixed(1));
        Zt.setAttribute("cy", (R0 + an * bi).toFixed(1));
        Zt.setAttribute("r", (5 * (1 - bn * 0.6) * mt).toFixed(2));
        Zt.setAttribute("opacity", (mt * 0.3 * (1 - bn)).toFixed(3));
      }
      const dn = this.rings[5];
      const on3 = clamp((Dt - 0.18) / 0.3, 0, 1), bn3 = on3 > 0 && on3 < 1;
      dn.style.display = bn3 ? "" : "none";
      if (bn3) {
        dn.removeAttribute("stroke-dasharray"); dn.removeAttribute("transform");
        dn.setAttribute("cx", `${R0}`); dn.setAttribute("cy", `${R0}`);
        dn.setAttribute("r", (20 + 34 * Rc(on3)).toFixed(1));
        dn.setAttribute("stroke-width", (2.8 * (1 - on3)).toFixed(2));
        dn.setAttribute("opacity", (mt * (1 - on3) * 0.8).toFixed(3));
      }
    }

    paintRecv(ze, now, stateAt) {
      const mt = Rc(ze), Dt = now - stateAt, Mt = Math.floor(Dt / RECV_MS);
      if (Mt !== this.recvTick) { this.recvTick = Mt; this.recvDir = rand(DIR_ARC[0], DIR_ARC[1]); }
      const Lt = (((Dt / RECV_MS) % 1) + 1) % 1, yn = clamp(Lt / 0.6, 0, 1), an = 1 - Math.pow(1 - yn, 3);
      const Et = Math.cos(this.recvDir), En = Math.sin(this.recvDir), Zt = 108 * (1 - an), dn = this.parts[5];
      const bn = yn < 1;
      dn.style.display = bn ? "" : "none";
      if (bn) {
        const Cn = 18 * Math.sin(yn * Math.PI) * (1 - an * 0.7);
        dn.setAttribute("cx", (R0 + Et * Zt + -En * Cn).toFixed(1));
        dn.setAttribute("cy", (R0 + En * Zt + Et * Cn).toFixed(1));
        dn.setAttribute("r", (3.5 + 6.5 * an).toFixed(2));
        dn.setAttribute("opacity", (mt * clamp(yn * 3.5, 0, 1) * (0.3 + 0.7 * an)).toFixed(3));
      }
      const on = this.rings[6];
      const bn2 = clamp((Lt - 0.58) / 0.32, 0, 1), Cn = bn2 > 0 && bn2 < 1;
      on.style.display = Cn ? "" : "none";
      if (Cn) {
        on.removeAttribute("stroke-dasharray"); on.removeAttribute("transform");
        on.setAttribute("cx", `${R0}`); on.setAttribute("cy", `${R0}`);
        on.setAttribute("r", (20 + 26 * Rc(bn2)).toFixed(1));
        on.setAttribute("stroke-width", (2.8 * (1 - bn2)).toFixed(2));
        on.setAttribute("opacity", (mt * (1 - bn2) * 0.8).toFixed(3));
      }
    }

    paintDock(ze, now, stateAt) {
      const mt = Rc(ze), Dt = (now - stateAt) / 1000, Mt = 42, Lt = 1.1;
      for (let yn = 0; yn < 2; yn++) {
        const an = this.parts[5 + yn];
        const Et = clamp((Dt - (0.2 + yn * 1.3)) / 0.9, 0, 1);
        if (Et <= 0) { an.style.display = "none"; continue; }
        const En = 1 - Math.pow(1 - Et, 3);
        const Zt = now * 0.001 * Lt + yn * Math.PI;
        const dn = R0 + Mt * Math.sin(Zt), on = R0 + Mt * 0.5 * Math.cos(Zt) + Math.sin(now * 0.003 + yn) * 2;
        const bn = R0 - 120 + yn * 30, Cn = R0 + 95;
        an.style.display = "";
        an.setAttribute("cx", (bn + (dn - bn) * En).toFixed(1));
        an.setAttribute("cy", (Cn + (on - Cn) * En).toFixed(1));
        an.setAttribute("r", ((7 + 3 * En) * mt).toFixed(2));
        an.setAttribute("opacity", (mt * clamp(Et * 4, 0, 1)).toFixed(3));
      }
    }

    paintPencil(ze, now, stateAt) {
      const mt = pencilPose(now, stateAt), Dt = this.glyphs[0];
      const Lt = ((mt.rot - 90) * Math.PI) / 180, yn = 68, an = Math.cos(Lt) * yn, Et = Math.sin(Lt) * yn;
      Dt.style.display = "";
      Dt.setAttribute("d", this.pencilPath);
      Dt.setAttribute("fill", this.fg);
      Dt.setAttribute("transform", `translate(${(R0 + (mt.x + an) * ze).toFixed(1)} ${(R0 + (mt.y + mt.wig * 0.15 + Et) * ze).toFixed(1)}) rotate(${(mt.rot * ze).toFixed(1)}) scale(${Rc(ze).toFixed(3)}) translate(${-R0} ${-R0})`);
      Dt.setAttribute("opacity", clamp(ze * 1.6 - 0.3, 0, 1).toFixed(3));
      if (ze > 0.6 && !mt.lift) {
        const x = R0 + mt.x, y = R0 + mt.y + mt.wig + 19, last = this.ink[this.ink.length - 1];
        if (!last || Math.hypot(x - last[0], y - last[1]) > 2.4) {
          this.ink.push([x, y]);
          if (this.ink.length > 64) this.ink.shift();
        } else { last[0] = x; last[1] = y; }
      } else if (this.ink.length) this.ink.splice(0, 2);
      const line = this.glyphs[1];
      if (this.ink.length < 2) line.style.display = "none";
      else {
        line.style.display = "";
        line.style.fill = "none";
        line.style.stroke = this.fg;
        line.setAttribute("stroke-width", "6");
        line.setAttribute("stroke-linecap", "round");
        line.setAttribute("stroke-linejoin", "round");
        line.setAttribute("d", smoothLine(this.ink));
        line.setAttribute("opacity", clamp(ze * 1.2, 0, 1).toFixed(3));
      }
    }

    paintBang(ze, now, stateAt) {
      const mt = this.glyphs[2];
      const Dt = (now - stateAt) / 1000, Mt = Rc(clamp(ze * 1.1, 0, 1));
      const Lt = Math.exp(-((Dt % 2.2) * 5.5)), yn = Math.sin(Dt * 42) * 2.2 * Lt;
      mt.style.display = "";
      mt.setAttribute("d", this.bangPath);
      mt.setAttribute("fill", this.fg);
      mt.setAttribute("transform", `translate(0 ${(-26 - (1 - Mt) * 70).toFixed(1)}) rotate(${yn.toFixed(2)} ${R0} ${(R0 - 74).toFixed(1)}) translate(${R0} ${R0}) scale(${clamp(ze * 1.2, 0, 1).toFixed(3)}) translate(${-R0} ${-R0})`);
      mt.setAttribute("opacity", clamp(ze * 1.5 - 0.2, 0, 1).toFixed(3));
    }

    paintStandby(ze) {
      const mt = Rc(ze), Dt = this.parts[4];
      const Lt = 0.5 + 0.5 * Math.sin(performance.now() * 0.0016);
      Dt.style.display = "";
      Dt.setAttribute("cx", `${R0}`); Dt.setAttribute("cy", `${R0}`);
      Dt.setAttribute("r", (26 + 7 * Lt).toFixed(1));
      Dt.setAttribute("opacity", (mt * (0.06 + 0.1 * Lt)).toFixed(3));
      const Mt = this.rings[2];
      const show = ze < 0.995;
      Mt.style.display = show ? "" : "none";
      if (show) {
        Mt.removeAttribute("stroke-dasharray"); Mt.removeAttribute("transform");
        Mt.setAttribute("cx", `${R0}`); Mt.setAttribute("cy", `${R0}`);
        Mt.setAttribute("r", (104 - 88 * mt).toFixed(1));
        Mt.setAttribute("stroke-width", "2.4");
        Mt.setAttribute("opacity", ((1 - mt) * 0.5).toFixed(3));
      }
    }

    extras(now, stateAt, cur, prev, yl, mix) {
      const kl = (name) => this.amount(name, cur, prev, yl, mix);
      const Lee = kl("dots");
      const rX = this.dotsPulse(now, 1, yl, this._reduce);
      let iX = 1;
      if (cur === "dots" || prev === "dots") iX = 1 + (rX.pop - 1) * (Lee / Math.max(yl, 0.001));
      const Fme = kl("receive");
      if (Fme > 0.004) {
        const _t = ((((now - stateAt) / RECV_MS) % 1) + 1) % 1, gn = clamp((_t - 0.58) / 0.34, 0, 1);
        iX *= 1 + 0.11 * Math.sin(gn * Math.PI) * Fme;
      }
      const zme = kl("send");
      if (zme > 0.004) {
        const _t = ((((now - stateAt) / SEND_MS) % 1) + 1) % 1;
        const gn = _t < 0.18 ? -0.06 * Math.sin((_t / 0.18) * Math.PI) : 0;
        const Gn = _t >= 0.18 && _t < 0.42 ? 0.05 * Math.sin(((_t - 0.18) / 0.24) * Math.PI) : 0;
        iX *= 1 + (gn + Gn) * zme;
      }
      const qee = kl("bang");
      if (qee > 0.004) iX *= 1 + 0.04 * Math.exp(-(((now - stateAt) / 1000) % 2.2) * 5.5) * qee;
      let yre = 0, aX = 0, wl = 0;
      const c1 = kl("pencil");
      if (c1 > 0.004) {
        const _t = pencilPose(now, stateAt);
        yre += _t.x * c1; aX += (_t.y + _t.wig * 0.5) * c1; wl += _t.rot * c1;
      }
      if (qee > 0.004) aX += 58 * qee;
      const jee = kl("whirl");
      if (jee > 0.004) {
        const _t = now / 1000;
        yre += (Math.sin(_t * 0.9) * 2 + Math.sin(_t * 1.7) * 0.8) * jee;
        aX += (Math.sin(_t * 1.3) * 2.4 + Math.sin(_t * 0.6) * 1.2) * jee;
      }
      const kre = kl("ball");
      if (kre > 0.004) {
        const _t = (now - stateAt) / 1000, gn = 0.62, Gn = 52, Ti = (8 * Gn) / (gn * gn), Ui = 40;
        const Si = Math.sqrt((2 * Ui) / Ti);
        let Ea;
        if (_t < Si) Ea = Ui - 0.5 * Ti * _t * _t;
        else {
          const Ca = (((_t - Si) / gn) % 1 + 1) % 1;
          Ea = 4 * Gn * Ca * (1 - Ca);
        }
        aX += (40 - Ea) * kre;
      }
      const mixR = (tbl) => {
        if (!cur) return 19;
        return (tbl[cur] || 19) * mix + (prev ? tbl[prev] || tbl[cur] : tbl[cur] || 19) * (1 - mix);
      };
      const A2 = mixR(RADIUS);
      const wre = (A2 / R0) * iX;
      const standby = kl("standby");
      const fade = standby > 0 ? (0.28 + 0.2 * Math.sin(now * 0.0016)) * standby : 0;
      const zoomCur = cur ? Math.max(SCALE[cur], 1) : 1;
      const zoomPrev = prev ? Math.max(SCALE[prev], 1) : zoomCur;
      const zoom = 1 + (zoomCur * mix + zoomPrev * (1 - mix) - 1) * yl;
      return { Lee, rX, yre, aX, wl, wre, fade, zoom, A2 };
    }

    resetInk() { this.ink = []; }
  }

  g.FOX_FX = { MAP, CYCLE, CYCLE_ON, CYCLE_OFF, SCALE, P_BLEND, R0, K, DIR_ARC, createParticles, OverlayLayer };
})(window);
