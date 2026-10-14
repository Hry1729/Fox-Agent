/* Fox port — L2: FoxAvatar. Full 39-state orchestration:
   eyes, gaze, blink, tricks (spin/bounce/dizzy/wild), overlays, particles.
   Timing/scheduling semantics mirrored from source $_t + sd().
   Trick offsets (yi/ki/hop) are in SOURCE space — composited with K = 360/Re0. */
(function (g) {
  const M = g.GROK_MATH;
  const {
    spring, stepSpring, springSteps, clamp, rand, sign, K2, mapPointer, Rn,
  } = M;
  const T = g.FOX_TABLES;
  const EY = g.FOX_EYES_MOD;
  const MO = g.FOX_MOTION;
  const FXF = g.FOX_FX;
  const R0 = FXF.R0;
  const K = FXF.K;
  /* 平滑阶梯（Hermite）：两端一阶导为 0 —— 过渡起止都不"起步就跳"。 */
  const smoothstep = (x, a, b) => {
    const t = clamp((x - a) / (b - a), 0, 1);
    return t * t * (3 - 2 * t);
  };
  const CLOSEUP_AT = T.CLOSEUP_AT;
  const DISK_R = 362; // fox-base.png 实心圆盘半径（PIL 实测 361.2）
  /* 两眼中点相对画布中心的距离（202.1px）：缩放支点滑到这里时，眼睛正好落到
     画布锚点上；同时它也是"圆盘中心相对取景圆心的偏移"的比例系数。 */
  const F_OFF = Math.hypot(CLOSEUP_AT[0] - 360, CLOSEUP_AT[1] - 360);

  class FoxAvatar {
    constructor(svg, opts = {}) {
      this.svg = svg;
      this.state = "idle";
      this.followPointer = opts.followPointer ?? true;
      this.autoTricks = opts.autoTricks ?? true;
      this.onChange = opts.onChange || (() => {});
      this.reduceMotion =
        opts.reduceMotion ??
        (typeof matchMedia === "function" &&
          matchMedia("(prefers-reduced-motion: reduce)").matches);

      /* 分项动画开关：呼吸 / 浮动 / 摇摆 / 挤压 */
      this.anim = { breath: true, float: true, roll: true, squash: true };

      this.morph = spring(1);
      this.blink = spring(1);
      this.eyeScale = spring(1);
      this.gazeX = spring(0);
      this.gazeY = spring(0);
      this.squash = spring(1);
      this.overlay = spring(0);
      this.overlayMix = spring(1);
      this.closeup = spring(0);
      this.humDots = spring(0);
      this.spinTurn = null;

      this.t0 = performance.now();
      this.stateAt = this.t0;
      this.last = this.t0;
      this.eyeFrom = 0;
      this.eyeTo = 0;
      this.liveFrom = null;
      this.eyeStiffness = T.SPRINGS.morph[0];
      this.eyeIdx = 0;
      this.eyeUntil = this.t0 + rand(...T.EYE_HOLD_MS.idle);
      this.blinkUntil = this.t0 + rand(1500, 7000);
      this.blinkQueue = [];
      this.gazeUntil = this.t0 + 800;
      this.winkAt = -1e9;
      this.winkEye = 0;
      this.winkUntil = this.t0 + rand(3000, 8000);
      this.hopAt = -1;
      this.hopNext = -1;
      this.trick = null;
      this.trickAt = this.t0 + rand(9000, 18000);
      this.celebrateAt = -1;
      this.ovSpin = 0;
      this.ovTarget = null;
      this.ovKind = null;
      this.ovPrev = null;
      this.ovRest = false;
      this.ovRestAt = 0;
      this.extras = { turn: null, Kr: 0, yi: 0, ki: 0, Yr: 0, Zr: 0, wi: 0, lidMul: null, hop: 0 };
      /* 眼部语言状态（笑眼弧线 / 爱心眼形 / 眼内装饰 / 高光绕圈 / 高光白心 / 斗鸡眼内聚）：
         见 tables.EYE_FX */
      this.arcMix = 0;
      this.arcType = null;
      this.heartMix = 0;
      this.symMix = 0;
      this.fxBoost = 1;
      this.glyph = { a: null, b: null, mix: 1 };
      this.orbitMix = 0;
      this.hlHeartMix = 0;
      this.crossMix = 0;
      /* 犯困：慢速点头式眨眼（睁 ↔ 闭 来回切换），见 tables.EYE_NOD */
      this.nodClose = 0;
      this.nodAsleep = false;
      this.fxTo = null;
      this.pointer = { x: 0, y: 0, tx: 0, ty: 0 };
      this.pointerRaw = null;
      this.rectCache = null;
      this.rectAt = -1e9;
      this.lastZoom = 1;

      this._build();
      this.setState("idle", true);
      this._bindPointer();
      this._raf = requestAnimationFrame((t) => this._tick(t));
    }

    destroy() {
      cancelAnimationFrame(this._raf);
      this._unbindPointer();
    }

    /* —— 写入去重：值没变就不碰 DOM ——
       每帧重复写同一个属性会持续触发样式重算/绘制失效（clip-path 尤甚，
       改一次就要重做整个组的裁剪与重绘）。缓存放在元素自身的私有键上，
       不影响测试台的 attrs/style 读数。 */
    _set(el, name, val) {
      if (!el) return;
      const k = "_v$" + name;
      if (el[k] === val) return;
      el[k] = val;
      el.setAttribute(name, val);
    }
    _css(el, name, val) {
      if (!el) return;
      const k = "_s$" + name;
      if (el[k] === val) return;
      el[k] = val;
      el.style[name] = val;
    }

    /* —— DOM：overlay 层（体下）→ 底图 PNG + 视线组（体上）→ 粒子层 —— */
    _build() {
      const ns = "http://www.w3.org/2000/svg";
      this.svg.setAttribute("viewBox", "0 0 720 720");
      this.svg.innerHTML = "";

      /* 皮肤覆盖（index-eyes / index-mini / index-blue 等变体页用）。
         必须**先读**：它同时决定 overlay 层配色、底图/补片配色与缩点形态。
         base：底图路径。蓝盘皮肤指向 assets/fox-base-blue.png —— 那张图的底盘
               已经被换成淡蓝（见 tables.DOT_BLUE 的注释），换肤不碰原图。
         dotMode："face"（默认）= 产品动作小点里是怼脸脸部特写；
                  "mini"（index-mini.html）= 最早那版「整只狐狸图片等比缩小成缩影」；
                  "blue"（index-blue.html）= 缩点直接变成**淡蓝实心圆**（见 DOT_BLUE）。
         后两者都不做怼脸取景 —— 见 _paint 里 fo 的置零。
         orn：把 overlay 的圆点/粒子/圈/字形从单一 var(--fg) 改成**多色交替**。
              true = 用 tables.ORN_BLUE（蓝盘主题：两格同色淡蓝 → 等价单色）；
              也可直接给 [色1, 色2, ...]。
         ornBurst：撒花粒子色板。true = 用 tables.ORN_BURST（蓝盘主题 = 单色淡蓝）。 */
      const skin = window.FOX_SKIN || {};
      this.dotMode = skin.dotMode || "face";
      this.dotBlue =
        this.dotMode === "blue" ? { ...T.DOT_BLUE, ...(skin.dotBlue || {}) } : null;
      const ornRaw = skin.orn;
      const orn =
        ornRaw === true
          ? T.ORN_BLUE
          : Array.isArray(ornRaw) && ornRaw.length >= 2
            ? ornRaw
            : null;
      const burstRaw = skin.ornBurst;
      this.ornBurst =
        burstRaw === true
          ? T.ORN_BURST
          : Array.isArray(burstRaw) && burstRaw.length
            ? burstRaw
            : null;
      this.fxLayer = new FXF.OverlayLayer(orn ? orn[0] : "var(--fg)", orn ? orn[1] : null);
      /* 蓝盘主题：周围小圆的"暗端下限"。源版跳动点脉冲最暗 0.5、环绕粒子背面最暗
         0.18 —— 在深底上后者直接读成**黑色小圆**（正是用户反馈里的"黑色小圆"），
         前者读成灰。把两个下限一起抬到同一个值，则任何时刻都读得出是淡蓝；
         深度感仍由半径差（环绕粒子 6→12 源空间）与脉冲节奏承担。
         不传（或写 0.5/0.18）即完全回到原版行为。 */
      if (typeof skin.ornFloor === "number") {
        this.fxLayer.orbitFloor = skin.ornFloor;
        this.fxLayer.dotFloor = skin.ornFloor;
      }
      this.svg.appendChild(this.fxLayer.root);

      /* 怼脸取景的圆形裁剪（屏幕空间固定圆心，半径随 cu 收缩）：
         内容放大后 PNG 蓝盘在屏幕上变大偏移，必须用 clip 圈掉出画的部分 */
      const defs = document.createElementNS(ns, "defs");
      const cp = document.createElementNS(ns, "clipPath");
      cp.setAttribute("id", "foxCloseupClip");
      this.clipCircle = document.createElementNS(ns, "circle");
      this.clipCircle.setAttribute("cx", "360");
      this.clipCircle.setAttribute("cy", "360");
      this.clipCircle.setAttribute("r", "380");
      cp.appendChild(this.clipCircle);
      defs.appendChild(cp);
      this.svg.appendChild(defs);

      this.outerG = document.createElementNS(ns, "g");
      this.outerG.setAttribute("clip-path", "url(#foxCloseupClip)");
      this.svg.appendChild(this.outerG);

      /* 皮肤覆盖（index-eyes.html 等变体页用）：FOX_SKIN 可替换底图/补片配色 ——
         已在 _build 开头读取（overlay 配色要先于 fxLayer 构造决定）。 */
      /* 白色补白（屏幕空间圆）：怼脸过渡时取景圆内、狐狸圆盘之外的区域要填白，
         否则露出页面背景 = 缩小/放大时边缘"缺角"。它铺在 moverG 之下、且与
         裁剪圆逐帧同形同参，正好只补"取景圆内没被内容盖住"的那部分。
         历史教训：早期用一块 2880² 的内容空间 rect —— SVG 的绘制范围被恒定
         撑到 2880×2881（实测 getBBox，每帧光栅 1.85Mpx，连静止态也一样），
         叠加 svg 的 overflow:visible，每帧重绘范围巨大 → 整个页面动画卡顿。
         现在半径 ≤R（约 60~512），且不需要时 display:none 彻底移出渲染树。 */
      const pad = document.createElementNS(ns, "circle");
      pad.setAttribute("cx", "360");
      pad.setAttribute("cy", "360");
      pad.setAttribute("r", "0");
      pad.setAttribute("fill", skin.chin || "#fafafc");
      pad.setAttribute("pointer-events", "none");
      pad.style.opacity = "0";
      pad.style.display = "none";
      this.padCircle = pad;
      this.outerG.appendChild(pad);

      const mkImg = (href) => {
        const im = document.createElementNS(ns, "image");
        im.setAttribute("href", href);
        im.setAttribute("x", "0");
        im.setAttribute("y", "0");
        im.setAttribute("width", "720");
        im.setAttribute("height", "720");
        return im;
      };
      /* —— 第 2 层：本体层（唯一的内容层）——
         保持原版语义：wre 缩点 + 编排位移/旋转/挤压；怼脸时再追加
         "支点滑到脸上 + 整体放大 z2"（见 _paint）。只有一份内容 → 不存在
         "复制一份放大后淡入"那种越界/错位：旧分层版的脸层比本体大
         (z2−1)×362×s，α 上升期间会从狐狸轮廓外露出一圈白毛 = 白色闪一下。 */
      this.moverG = document.createElementNS(ns, "g");
      this.outerG.appendChild(this.moverG);
      this.contentG = document.createElementNS(ns, "g");
      this.moverG.appendChild(this.contentG);
      this.contentG.appendChild(mkImg(skin.base || "assets/fox-base.png"));

      /* 特写纯色盖板（皮肤项 closeupFill，index-eyes.html 用）：怼脸收满前后把整张脸
         压成与粒子同色的纯色，只留两只眼睛（眼睛在 eyeG 层，恒在其上）。
         挂在本体层里（底图之上）→ 与本体同变换、被取景圆裁掉圈外部分，
         不可能像早期版本那样"盖板边缘露到盘外"。 */
      this.closeupFill = null;
      if (skin.closeupFill) {
        const cf = document.createElementNS(ns, "rect");
        cf.setAttribute("x", "0");
        cf.setAttribute("y", "0");
        cf.setAttribute("width", "720");
        cf.setAttribute("height", "720");
        cf.setAttribute("fill", skin.closeupFill);
        cf.setAttribute("pointer-events", "none");
        cf.style.opacity = "0";
        cf.style.display = "none";
        this.closeupFill = cf;
        this.contentG.appendChild(cf);
      }

      /* 眼簇独立成层（不被盖板/粒子遮住，始终清晰）：变换与本体层逐帧同参，
         所以眼睛不可能与脸错位；静止态该变换就是原版本体变换。 */
      this.eyeG = document.createElementNS(ns, "g");
      this.outerG.appendChild(this.eyeG);
      const built = EY.buildEls(this.eyeG);
      this.gazeG = built.gazeG;
      this.faceG = built.faceG;
      this.eyeEls = built.els;
      this.hlEls = built.hls;
      this.hlHeartEls = built.hlHearts;
      this.decos = built.decos;
      /* 鼻子：底图实测的椭圆，挂在脸坐标系里（与眼睛同变换、但不跟视线平移），
         平时藏起来 —— 底图自带鼻子；只有特写态淡入，用来在"底图被纯色盖板压白"
         之后把鼻子补回来。 */
      this.noseEl = built.nose;

      /* 蓝盘主题（dotMode="blue"）：缩点后盖住"整只狐狸"的那枚淡蓝实心圆。
         位置很关键 —— 挂在**眼层之上**：眼睛在本层之上单独绘制，圆若压在眼层下，
         缩完的小圆里会继续留着两个黑点，那还是"小头像"，不是纯色圆。
         变换与本体层逐帧同参（和 eyeG 一样），半径 = 实测盘半径 → 与底图底盘
         逐像素重合；不吃 bodyOp（色调衰减是给"狐狸本体"的，纯色圆必须实心）。 */
      this.dotG = null;
      this.dotDisc = null;
      if (this.dotBlue) {
        const dg = document.createElementNS(ns, "g");
        this.outerG.appendChild(dg);
        const dc = document.createElementNS(ns, "circle");
        dc.setAttribute("cx", "360");
        dc.setAttribute("cy", "360");
        dc.setAttribute("r", String(this.dotBlue.r));
        dc.setAttribute("fill", this.dotBlue.fill);
        dc.setAttribute("pointer-events", "none");
        dc.style.opacity = "0";
        dc.style.display = "none";
        dg.appendChild(dc);
        this.dotG = dg;
        this.dotDisc = dc;
      }

      this.partG = document.createElementNS(ns, "g");
      this.svg.appendChild(this.partG);
      this.particles = FXF.createParticles({
        back: this.partG,
        palette: this.ornBurst || undefined,
        star: this.ornBurst ? this.ornBurst[this.ornBurst.length - 1] : undefined,
      });
    }

    /* —— 状态机 —— */
    setState(name, reset = false) {
      if (!T.EYE_PLAYLIST[name]) return;
      this.state = name;
      this.stateAt = performance.now();
      const list = T.EYE_PLAYLIST[name];
      const body = T.BODY[name];
      this.eyeIdx = 0;
      if (reset) {
        this.eyeFrom = list[0];
        this.eyeTo = list[0];
        this.liveFrom = null;
        this.morph.x = 1;
        this.morph.t = 1;
        this.morph.v = 0;
      } else if (name === "sleeping") {
        /* 源 forceSleepEye：入睡直接闭眼 */
        this._morphEyes(13, 11);
      } else if (name !== "waking") {
        this._morphEyes(list[0], name === "excited" ? 10 : 8);
      }
      /* 眼部语言目标（未声明的状态 → null → 装饰/弧线全关、视线回原 GAZE 表） */
      this.fxTo = T.EYE_FX[name] || null;
      this._woke = false;
      this.eyeUntil = this.stateAt + rand(...T.EYE_HOLD_MS[name]);
      const blink = T.BLINK_MS[name];
      this.blinkUntil = blink ? this.stateAt + rand(1500, 7000) : Infinity;
      this.gazeUntil = this.stateAt + rand(500, 1400);
      this.winkUntil = this.stateAt + rand(3000, 8000);
      this.celebrateAt = name === "celebrate" ? this.stateAt + 140 : -1;
      this.trick = null;
      this.spinTurn = null;
      this.hopAt = -1;
      /* 犯困的点头相位从状态起点重新开始（周期用 now − stateAt 算） */
      this.nodClose = 0;
      this.nodAsleep = false;
      this.hopNext = body.autoHop
        ? this.stateAt + rand(...body.autoHop)
        : -1;
      this.ovSpin = 0;
      this.ovTarget = "__keep__"; // 强制 _stepOverlay 重算
      if (name !== "writing") this.fxLayer.resetInk();
      /* 入场那一帧排的眨眼：睡着/苏醒/犯困不眨（源语义），
         "弧线接管眼形"的状态（开心）也跳过 —— 眨眼会把眼睛**瞬间**压成一条缝，
         和"先变扁、再变弯"的衔接叠在一起就是"糊了一下"（用户反馈的那种突然感）。 */
      const arcEntry = !!(this.fxTo && this.fxTo.arc);
      if (name !== "waking" && name !== "sleeping" && name !== "drowsy" && !arcEntry) {
        EY.queueBlink(this.blinkQueue, this.stateAt);
      }
      if (name === "celebrate" && !this.reduceMotion) {
        this.particles.burst(22, 1.1, 0.3);
      }
      this.onChange(this.snapshot());
    }

    snapshot() {
      return {
        state: this.state,
        eyeFrom: this.eyeFrom,
        eyeTo: this.eyeTo,
        squash: this.squash.x,
        spin: (this.spinTurn ? this.spinTurn.x : 0) + (this.extras.turn || 0),
        overlay: this.ovKind,
      };
    }

    setFollowPointer(v) {
      this.followPointer = !!v;
      if (!v) this.pointerRaw = null;
    }

    setAutoTricks(v) {
      this.autoTricks = !!v;
    }

    setAnim(key, v) {
      if (key in this.anim) this.anim[key] = !!v;
    }

    /* —— 特技 API —— */
    spinOnce(turns = 1) {
      if (this.reduceMotion || this.spinTurn) return;
      this.spinTurn = MO.makeSpinTurn(turns, sign());
    }

    bounceOnce() {
      if (!this.reduceMotion && !this.trick) this.trick = MO.startTrick("spinBounce", false);
    }

    dizzyOnce() {
      if (!this.reduceMotion && !this.trick) this.trick = MO.startTrick("spinDizzy", false);
    }

    wildOnce() {
      if (!this.reduceMotion && !this.trick) this.trick = MO.startTrick("spinWild", false);
    }

    hop() {
      if (this.hopAt < 0) {
        this.hopAt = performance.now();
        if (this.anim.squash) this.squash.v -= 2.6; // 起跳前下压
      }
    }

    blinkOnce() {
      EY.queueBlink(this.blinkQueue, performance.now());
    }

    burstOnce(n = 20, h = 1, gr = 0) {
      if (!this.reduceMotion) this.particles.burst(n, h, gr);
    }

    /* —— 指针 —— */
    _bindPointer() {
      this._onMove = (e) => {
        if (!this.followPointer) return;
        this.pointerRaw = { x: e.clientX, y: e.clientY };
      };
      this._onLeave = () => {
        if (this.followPointer) this.pointerRaw = null;
      };
      window.addEventListener("pointermove", this._onMove, { passive: true });
      document.documentElement.addEventListener("pointerleave", this._onLeave);
    }

    _unbindPointer() {
      window.removeEventListener("pointermove", this._onMove);
      document.documentElement.removeEventListener("pointerleave", this._onLeave);
    }

    _updatePointer(now) {
      const src = this.followPointer ? this.pointerRaw : null;
      if (src && this.svg.getBoundingClientRect) {
        if (now - this.rectAt > 200) {
          this.rectCache = this.svg.getBoundingClientRect();
          this.rectAt = now;
        }
        const rect = this.rectCache;
        if (rect && rect.width > 0) {
          const mapped = mapPointer(rect, src);
          this.pointer.tx =
            clamp((mapped.x - (rect.left + rect.width / 2)) / rect.width, -0.6, 0.6) * 26;
          this.pointer.ty =
            clamp((mapped.y - (rect.top + rect.height / 2)) / rect.height, -0.6, 0.6) * 16;
        }
      } else {
        this.pointer.tx = 0;
        this.pointer.ty = 0;
      }
      const z = Rn(0.16);
      this.pointer.x += (this.pointer.tx - this.pointer.x) * z;
      this.pointer.y += (this.pointer.ty - this.pointer.y) * z;
    }

    /* —— morph 调度（源 _morphEyes 语义：捕获当前插值中的多边形作为新起点） —— */
    _morphEyes(index, stiffness = 7) {
      if (index === this.eyeTo && this.morph.t === 1) return;
      const t = clamp(this.morph.x, 0, 1);
      this.eyeFrom = this.eyeTo;
      this.liveFrom = EY.livePolys(this.eyeFrom, this.eyeTo, t, this.liveFrom);
      this.eyeTo = index;
      this.morph.x = 0;
      this.morph.v = 0;
      this.morph.t = 1;
      this.eyeStiffness = stiffness;
    }

    /* —— 犯困：慢速点头式眨眼（眼睛在"睁"与"闭"之间来回切换）——
       相位由"进入本状态的时间"算起 → 每次切到犯困都是从头开始的完整周期。
       波形与参数见 tables.EYE_NOD。 */
    _stepNod(now, dt) {
      const N = T.EYE_NOD;
      if (!(this.fxTo && this.fxTo.nod)) {
        /* 非犯困态：把闭眼进度指数放回 0 —— 否则离开时眼皮会一直卡在闭着的值上。
           用平滑回落而不是直接清零：眼睛是"慢慢睁开"着离开的，不会跳。 */
        this.nodClose += (0 - this.nodClose) * (1 - Math.exp(-9 * dt));
        this.nodAsleep = false;
        return;
      }
      const u = (((now - this.stateAt) / 1000) / N.period) % 1;
      let d;
      if (u < N.awake) d = 0;
      else if (u < N.closeEnd) d = smoothstep(u, N.awake, N.closeEnd);
      else if (u < N.shutEnd) d = 1;
      else d = 1 - smoothstep(u, N.shutEnd, 1);
      this.nodClose = d;
      /* 跨过半程时换眼形：闭走低刚度（慢慢沉）、睁走高刚度（一下弹开） */
      const asleep = d > N.flip;
      if (asleep !== this.nodAsleep) {
        this.nodAsleep = asleep;
        this._morphEyes(
          asleep ? N.shutIdx : N.openIdx,
          asleep ? N.droopStiff : N.wakeStiff
        );
      }
    }

    /* —— overlay 调度（源 _stepOverlay verbatim，去掉入场旋轮） —— */
    _stepOverlay(now) {
      /* 产品态特效照常播放（怼脸特写时 fox._paint 会抵消 overlay 对本体的作用）。
         无专属特效的产品态（哼歌）也要拉起 overlay 弹簧——缩点/特写由 yl 驱动，
         kind 为 null 时 fxLayer 不画粒子，音符双点+摇摆即其动画。 */
      const want = FXF.MAP[this.state] || null;
      const wantOn = want != null || T.CLOSEUP.has(this.state);
      if (want !== this.ovTarget) {
        this.ovTarget = want;
        this.fxLayer.overlayAt = now;
        this.ovRest = false;
        this.ovRestAt = 0;
      }
      let on = wantOn;
      if (want && FXF.CYCLE.has(this.state)) {
        if (!this.ovRest && now - this.fxLayer.overlayAt > (FXF.CYCLE_ON[this.state] || 2500)) {
          this.ovRest = true;
          this.ovRestAt = now;
        } else if (this.ovRest && now - this.ovRestAt > FXF.CYCLE_OFF) {
          this.ovRest = false;
          this.fxLayer.overlayAt = now;
        }
        on = !this.ovRest;
      }
      this.overlay.t = on ? 1 : 0;
      if (want && want !== this.ovKind) {
        if (this.ovKind && this.overlay.x > 0.02) {
          this.ovPrev = this.ovKind;
          this.overlayMix.x = 0;
          this.overlayMix.v = 0;
          this.overlayMix.t = 1;
        } else {
          this.ovPrev = null;
          this.overlayMix.x = 1;
          this.overlayMix.v = 0;
          this.overlayMix.t = 1;
        }
        this.ovKind = want;
        this.fxLayer.overlayAt = now;
        if (want !== "pencil") this.fxLayer.resetInk();
      }
      if (!want && this.overlay.x < 0.004) {
        this.ovKind = null;
        this.ovPrev = null;
      }
      if (this.overlayMix.x > 0.996) this.ovPrev = null;
    }

    /* —— 眼部语言推进（弧线 / 爱心眼形 / 装饰 / 同形等大 / 放大增益 / 斗鸡眼）——
       这几个量只做"表情收放"，用指数平滑（1 − e^(−k·dt)）而不是弹簧：不需要回弹、
       且天然与帧率无关、永不过冲。 */
    _stepEyeFX(dt) {
      const to = this.fxTo;
      const e = 1 - Math.exp(-T.EYE_FX_STIFF * dt);
      /* 笑眼弧线（淡出期间保留上一次的类型，否则弧线会在中途变形成另一种弯）。
         刚度单独取 EYE_ARC.stiff（比其它表情量慢）—— "先变扁再变弯"这个衔接
         需要看得清，用 7 的话 0.1 秒就冲完了。 */
      if (to && to.arc) this.arcType = to.arc;
      const eArc = 1 - Math.exp(-T.EYE_ARC.stiff * dt);
      this.arcMix += ((to && to.arc ? 1 : 0) - this.arcMix) * eArc;
      /* 爱心眼形接管 + 强制两眼同形等大 + 放大增益 */
      this.heartMix += ((to && to.shape === "heart" ? 1 : 0) - this.heartMix) * e;
      this.symMix += ((to && to.sym ? 1 : 0) - this.symMix) * e;
      this.fxBoost += ((to && to.boost ? to.boost : 1) - this.fxBoost) * e;
      /* 高光点绕圈（困惑）+ 高光点变白心（害羞） */
      this.orbitMix += ((to && to.orbit ? 1 : 0) - this.orbitMix) * e;
      this.hlHeartMix += ((to && to.hlHeart ? 1 : 0) - this.hlHeartMix) * e;
      /* 斗鸡眼内聚 */
      this.crossMix += ((to && to.look === "cross" ? 1 : 0) - this.crossMix) * e;
      /* 眼内装饰：A/B 交叉溶解（旧的转 B 淡出、新的进 A 淡入 → 天然连形过渡）。
         特技"眩晕转"期间强制叠漩涡眼 —— 转得七荤八素时眼睛也该是晕的。 */
      const dizzy = !!(this.trick && this.trick.kind === "spinDizzy");
      const gt = dizzy ? "swirl" : to ? to.glyph || null : null;
      if (gt !== this.glyph.a) {
        this.glyph.b = this.glyph.a;
        this.glyph.a = gt;
        this.glyph.mix = 0;
      }
      this.glyph.mix = clamp(this.glyph.mix + dt * 4.5, 0, 1);
      if (this.glyph.mix >= 1 && this.glyph.b != null) this.glyph.b = null;
    }

    _tick(now) {
      /* dt 夹到 [0, 0.1]：rAF 时间戳理论上单调，但页面从 bfcache 恢复 / 调试时
         手动推帧都可能给出"比上一帧更早"的 now —— 负 dt 会让 _stepEyeFX 的指数
         平滑反向放大（而且 glyph.mix 是按 dt 累加的，会被拉回负数 → 装饰莫名消失）。 */
      const dt = Math.min(Math.max((now - this.last) / 1000, 0), 0.1);
      this.last = now;
      this._dt = dt;
      const mt = (now - this.t0) / 1000;
      const dtState = (now - this.stateAt) / 1000;
      const body = T.BODY[this.state];

      /* 苏醒：短暂闭眼后睁眼 + 眨一下 */
      if (this.state === "waking" && !this._woke && dtState > 0.45) {
        this._woke = true;
        this._morphEyes(0, 8);
        EY.queueBlink(this.blinkQueue, now);
      }

      /* 眼睛播放列表推进（睡着/苏醒不推进，源语义；犯困由 nod 驱动 → 也不推进） */
      const nodding = !!(this.fxTo && this.fxTo.nod);
      if (
        this.state !== "waking" && this.state !== "sleeping" && !nodding &&
        now >= this.eyeUntil
      ) {
        const list = T.EYE_PLAYLIST[this.state];
        this.eyeIdx = (this.eyeIdx + 1 + Math.floor(rand(0, list.length - 1))) % list.length;
        const stiff = this.state === "searching" || this.state === "excited" ? 10 : 6;
        this._morphEyes(list[this.eyeIdx], stiff);
        this.eyeUntil = now + rand(...T.EYE_HOLD_MS[this.state]);
      }

      /* 犯困：慢速点头式眨眼（睁 ↔ 闭 来回切换） */
      this._stepNod(now, dt);

      /* 眨眼 */
      const blinkCadence = T.BLINK_MS[this.state];
      if (blinkCadence && now >= this.blinkUntil) {
        EY.queueBlink(this.blinkQueue, now);
        this.blinkUntil = now + rand(...blinkCadence);
      }
      const blinkKey = EY.consumeBlink(this.blinkQueue, now);
      if (blinkKey != null) this.blink.t = blinkKey; // 源语义：队列值 → 眨眼弹簧目标

      /* 视线：指针优先；其次按状态的眼部语言（LOOK：扫视/斜睨/上翻/躲闪/抖视）
         —— 这条比原版随机游移"更有性格"：好奇状态是真的在左右来回看，
         贪玩状态是绕小圈看，无聊状态是持续上翻。未声明 LOOK 的状态走原版游移。 */
      const hasPtr = !!(this.followPointer && this.pointerRaw);
      if (!hasPtr) {
        const look = this.fxTo ? this.fxTo.look : null;
        const cont = look ? T.LOOK_CONT[look] : null;
        if (cont) {
          /* 连续模式：按相位逐帧取值（不是"定个目标等弹簧过去"），于是永不停顿 */
          const w = mt * Math.PI * 2 * cont.f;
          this.gazeX.t = cont.ax * Math.sin(w);
          this.gazeY.t = look === "circle" ? cont.ay * Math.cos(w) : cont.by;
        } else if (look && T.LOOK[look]) {
          if (now >= this.gazeUntil) {
            const gz = T.LOOK[look]();
            this.gazeX.t = gz.x;
            this.gazeY.t = gz.y;
            this.gazeUntil = now + rand(...gz.hold);
          }
        } else if (now >= this.gazeUntil) {
          const gz = T.GAZE[this.state]();
          this.gazeX.t = gz.x;
          this.gazeY.t = gz.y;
          this.gazeUntil = now + rand(...gz.hold);
        }
      }
      this._updatePointer(now);

      /* 眼部语言推进（弧线 / 装饰 / 斗鸡眼） */
      this._stepEyeFX(dt);

      /* overlay 调度 */
      this._stepOverlay(now);

      /* 怼脸特写弹簧目标 */
      this.closeup.t = T.CLOSEUP.has(this.state) && !this.reduceMotion ? 1 : 0;

      /* 庆祝：周期性狂野转 + 撒花 */
      if (this.celebrateAt > 0 && now >= this.celebrateAt && !this.trick && !this.spinTurn) {
        this.trick = MO.startTrick("spinWild", this.reduceMotion);
        this.celebrateAt = now + 6200;
      }

      /* 自动特技：V/B 状态走源版概率菜单；autoHop 状态定时跳 */
      if (this.autoTricks && now >= this.trickAt) {
        if (
          (T.V_T.has(this.state) || T.B_T.has(this.state)) &&
          !this.spinTurn && this.hopAt < 0 && !this.trick && !this.reduceMotion
        ) {
          const z = Math.random();
          if (T.V_T.has(this.state)) {
            if (z < 0.55) this.spinOnce(1);
            else this.trick = MO.startTrick("spinBounce", this.reduceMotion);
          } else if (z < 0.34) this.trick = MO.startTrick("spinBounce", this.reduceMotion);
          else if (z < 0.62) this.hop();
          else if (z < 0.86) this.trick = MO.startTrick("spinDizzy", this.reduceMotion);
          else this.spinOnce(1);
        }
        this.trickAt = now + rand(9000, 18000);
      }
      if (this.autoTricks && body.autoHop && this.hopNext > 0 && now >= this.hopNext) {
        if (this.hopAt < 0 && !this.spinTurn && !this.trick) this.hop();
        this.hopNext = now + rand(...body.autoHop);
      }

      /* wink */
      if (T.WINK_STATES.has(this.state) && now >= this.winkUntil) {
        this.winkAt = now;
        this.winkEye = Math.random() < 0.5 ? 0 : 1;
        this.winkUntil = now + rand(4500, 10000);
      }

      /* 特技求值（源 verbatim） */
      const tf = MO.evalTrick(this.trick, now);
      if (tf.wantHop) this.hop();
      if (tf.done) this.trick = null;
      let hop = MO.hopY(this.hopAt, now);
      if (hop == null) {
        this.hopAt = -1;
        hop = 0;
      }
      this.extras = { ...tf, hop };

      /* humming/loading 自旋（源 ovSpin 节奏） */
      const humming = this.state === "humming";
      const loading = this.state === "loading";
      if ((humming || loading) && !this.reduceMotion) {
        const Zt = dtState;
        const dn = loading ? 3 : 1.6;
        const on = Zt < 0.5 ? 7 * K2(Zt / 0.5) : Zt < 1.3 ? 7 + (dn - 7) * K2((Zt - 0.5) / 0.8) : dn + 0.3 * Math.sin(Zt * 0.5);
        /* ovSpin 只作摇摆/音符相位（不再直接旋转本体），无需衰减 */
        this.ovSpin += on * dt;
      }

      /* 弹簧积分 */
      const nSteps = springSteps(dt);
      const step = dt / nSteps;
      for (let i = 0; i < nSteps; i++) {
        stepSpring(this.morph, this.eyeStiffness, 1, step);
        stepSpring(this.blink, ...T.SPRINGS.blink, step);
        stepSpring(this.eyeScale, ...T.SPRINGS.eyeScale, step);
        stepSpring(this.gazeX, ...T.SPRINGS.gazeX, step);
        stepSpring(this.gazeY, ...T.SPRINGS.gazeY, step);
        if (this.anim.squash) stepSpring(this.squash, ...T.SPRINGS.squash, step);
        if (this.spinTurn) stepSpring(this.spinTurn, ...T.SPRINGS.spinTurn, step);
        stepSpring(this.overlay, ...T.SPRINGS.overlay, step);
        stepSpring(this.overlayMix, ...T.SPRINGS.overlayMix, step);
        stepSpring(this.closeup, ...T.SPRINGS.closeup, step);
        stepSpring(this.humDots, 6, 1, step);
      }
      if (this.spinTurn && MO.spinTurnSettled(this.spinTurn)) this.spinTurn = null;
      if (!this.anim.squash || this.reduceMotion) {
        this.squash.x = 1;
        this.squash.v = 0;
      }
      if (this.reduceMotion) {
        this.blink.x = 1;
      }
      this.humDots.t = humming ? 1 : 0;

      this._paint(now, mt, hop, hasPtr, blinkKey);
      this._raf = requestAnimationFrame((t) => this._tick(t));
    }

    _paint(now, mt, hop, hasPtr, blinkKey) {
      const body = T.BODY[this.state];
      const yl = clamp(this.overlay.x, 0, 1);
      const mix = clamp(this.overlayMix.x, 0, 1);
      this.fxLayer._reduce = this.reduceMotion;
      const ov = this.fxLayer.extras(now, this.stateAt, this.ovKind, this.ovPrev, yl, mix);
      const bodyW = 1 - yl;
      const ex = this.extras;
      const cu = clamp(this.closeup.x, 0, 1);

      /* 分项身体动画（可被开关关闭） */
      const bt = mt * body.breath[1] * Math.PI * 2;
      const breath =
        this.reduceMotion || !this.anim.breath ? 0 : body.breath[0] * Math.sin(bt);
      const roll =
        this.reduceMotion || !this.anim.roll
          ? 0
          : body.roll[0] * Math.sin(mt * body.roll[1] * Math.PI * 2);
      const tyFloat =
        this.reduceMotion
          ? body.ty[2] * 0.5
          : this.anim.float
            ? body.ty[2] + body.ty[0] * Math.sin(mt * body.ty[1] * Math.PI * 2)
            : body.ty[2];

      /* 容器合成（源 _paint 变换语义，fox 空间）——产品态 verbatim：
         wre 缩点、yre/aX/wl/lift 编排、相机 zoom 全部走原版路径。
         怼脸 = 在同一层上追加两个连续量，都由 fo = min(cu, yl) 驱动：
           ① 缩放支点从画布中心滑到两眼中点（进度由 CLOSEUP_SHIFT 曲线反解）→
              眼睛最终落在
              画布锚点上，点的位置与原版蓝点一致（旧版把脸钉在取景圆心上，
              圆盘跟着脸被拽到图片下方 = 产品动作整体偏下 ~30px）；
           ② 内容放大 z2（CLOSEUP_ZOOM_IN，起得晚）→ 取景圆里从"蓝点"变成
              "脸部白毛 + 眼睛"，蓝圈与耳朵被推出圈外。
         cu 是状态门控（仅 12 个产品态），取 min 保证：CYCLE 休息相回归完整狐狸
         （原版节奏）、thinking 等非产品态永远保持原版蓝点形态。 */
      /* dotMode="mini"（index-mini.html）：小点形态 = 「整只狐狸图片等比缩小」
         （原版 verbatim 的蓝盘缩影），不用怼脸取景。
         dotMode="blue"（index-blue.html）：同上不减内容，只在缩点后半程漫上一枚
         淡蓝实心圆（dotG），小点于是读作"一枚淡蓝圆"而不是"一只小狐狸"。
         两者特写量全部由 fo 驱动，置 0 后 z2/支点/取景圆/白垫底/盖板/鼻子/视线
         衰减统一走静止路径 —— 恰好就是第一版产品动作；缩点编排（wre/yre/aX/wl/
         lift）与表情系统不动。 */
      const fo = this.dotMode === "face" ? Math.min(cu, yl) : 0;
      const eFill = smoothstep(fo, ...T.CLOSEUP_FADE.face);
      const tx = ex.yi * bodyW * K + ov.yre * yl * K;
      /* 点形态（yl→1）下 bodyW→0 会吞掉本体空间动画，需独立编排通道：
         哼歌/加载把 ovSpin 相位转成"点头晃脑"的小幅摇摆+浮动
         （源版是持续自旋，脸持续旋转观感差）；弹跳的 hop 由 bodyW 项
         + yl 项合成，点形态下恢复满幅跳跃。 */
      const isHum = this.state === "humming";
      const isLoad = this.state === "loading";
      const dotSway =
        isHum || isLoad ? Math.sin(this.ovSpin) * (isHum ? 9.5 : 6.5) * yl : 0;
      const dotBob =
        isHum || isLoad
          ? Math.sin(this.ovSpin * 0.55 + 0.9) * (isHum ? 14 : 9) * yl
          : 0;
      const ty =
        (tyFloat + hop * K) * bodyW +
        hop * K * yl +
        dotBob +
        ex.ki * bodyW * K -
        ov.rX.lift * ov.Lee * K +
        ov.aX * yl * K;
      const bodyRot = body.tilt + roll;
      /* 特技旋转：转整个头像（狐狸是位图身体，眼睛在脸上打转不合逻辑） */
      const turnDeg = ex.turn != null ? (ex.turn * 180) / Math.PI : 0;
      const spinDeg = this.spinTurn ? (this.spinTurn.x * 180) / Math.PI : 0; // 弧度→度
      const rot =
        (bodyRot + ex.Kr + turnDeg) * bodyW +
        ov.wl * yl +
        spinDeg +
        dotSway;
      const sx = (1 - breath * 0.6) * (bodyW + ov.wre * yl);
      const sy = (1 + breath) * (this.squash.x * bodyW + ov.wre * yl);
      /* 内容放大系数 z2：前半程保持原版"缩点"形态（此刻裁剪还是空操作，
         读起来就是镜头在推进），后半程才把蓝圈/耳朵推出取景圆、露出脸部白毛。 */
      const z2 = 1 + T.CLOSEUP_ZOOM * smoothstep(fo, ...T.CLOSEUP_ZOOM_IN);
      const sAvg = (Math.abs(sx) + Math.abs(sy)) / 2;
      const [F0, F1] = CLOSEUP_AT;
      /* 支点进度 tp 由"想要的偏心量 d"反解，而不是直接写一条 fo 曲线：
           d = |Sx × (360 − pv)| = Sx × 202 × tp          （几何恒等式）
         令 d = 202 × 终态尺度 × g(fo)，解出
           tp = 终态尺度 × g / Sx                          （clamp 到 ≤1）
         ⇒ 位移走的是一条与 Sx 无关、单调、可从第一帧起步的曲线 g(fo)。
         终态时 Sx→终态尺度、g→1 ⇒ tp→1：支点正好落在两眼中点，眼睛精确停在
         锚点上（= 其它状态头像的中心）。
         对照：直接写 tp = smoothstep(fo) 时 d = Sx×202×tp 被本体缩点牵着 ——
         早期 Sx≈1 而 tp≈0 → 位移≈0，观感是"都已经缩成最小圆了才突然往右上
         蹦一下"（实测前 14 帧 R 从 511 掉到 129 而位移全程为 0）；想把 tp
         提前，乘积又会让 d 在途中冲过终态值 → 先飘出去再弹回来。 */
      const g = smoothstep(fo, ...T.CLOSEUP_SHIFT);
      /* 分母必须是"作用在变换上的尺度" = sAvg × z2（baseT 用的是 scale(sx*z2…)）；
         少乘 z2 会让 tp 提早饱和到 1 → d = 202×sAvg×z2×1 冲到 58 再回落 53 =
         先往外飘一下再弹回来。
         标定值 0.26 是"典型终态尺度"，而各状态的终态尺度有小幅差异
         （0.18 ~ 0.28）：偏大的那几个（sending/receiving/uploading）终态 tp
         只到 0.93，眼睛会残留 ~2px 屏幕偏移。乘一个只在 fo 0.8→1 生效的收敛
         因子把 tp 拉到 1 即可 —— 这一段 d 本就已趋稳，不会破坏单调。 */
      const tpRaw = clamp((T.CLOSEUP_SHIFT_TO * g) / Math.max(sAvg * z2, 1e-4), 0, 1);
      const tp = tpRaw + (1 - tpRaw) * smoothstep(fo, 0.8, 1);
      const pvx = 360 + tp * (F0 - 360);
      const pvy = 360 + tp * (F1 - 360);
      /* 取景圆（根空间）：圆心恒等于头像锚点（360+tx, 360+ty）——它同时是
         "点和别的状态同心"的唯一参照；半径 = 本体尺度 × 视野系数（512→352 随
         fo 收紧，收满 ≈352×sAvg = 圆盘半径 = 原版蓝点尺寸）。
         fo≤0.002 撤销裁剪：512×sAvg 已完整包住画布内容，这个开关不可见。 */
      const R = sAvg * (T.CLOSEUP_FRAME[0] + (T.CLOSEUP_FRAME[1] - T.CLOSEUP_FRAME[0]) * fo);
      const ccx = 360 + tx;
      const ccy = 360 + ty;
      /* 裁剪相关的属性都走 _set 去重：clip-path 每帧重写＝每帧重做整组裁剪+重绘，
         而在特写稳态下它和半径其实都是常量，没必要逐帧刷新。 */
      this._set(
        this.outerG,
        "clip-path",
        fo > 0.002 ? "url(#foxCloseupClip)" : "none"
      );
      this._set(this.clipCircle, "r", R.toFixed(2));
      this._set(this.clipCircle, "cx", ccx.toFixed(2));
      this._set(this.clipCircle, "cy", ccy.toFixed(2));
      /* 白垫底（与取景圆逐帧同形）：只负责把"取景圆内、圆盘轮廓之外"那一瓣月牙
         补成白 —— 于是点的形状恒为取景圆，而不是"圆盘∩取景圆"的缺口形状
         （那就是早前的"缺角"）。
         α 的判据全由几何给出，不写死 fo 曲线：
           diskR   = 362 × sAvg × z2           放大后圆盘半径
           diskOff = sAvg × z2 × tp × 202      圆盘中心相对取景圆心的偏移
         R ≥ diskR+diskOff：取景圆包住圆盘 → 裁剪是空操作 → α 必须为 0
           （否则狐狸外面糊一圈白 = 用户说的"白色闪了一下"）；
         R ≤ diskR−diskOff：取景圆整个进到圆盘里 → 月牙最宽 → α 为 1。
         中间只在 CLOSEUP_PAD_W 这么宽的"切入深度"里爬升，其余时间恒为 1。
         注意 PAD_W 窗口的自变量正是"露白宽度"本身，所以不管位移（diskOff）
         从哪一帧开始爬升，α 都恰好跟着露白一起长 —— 不存在"该露白处从深色
         瞬变白"的跳变。位移曲线因此可以彻底自由（CLOSEUP_SHIFT 铺满全程）。
         旧版把 α 写成 fo 的固定曲线（0.12→0.42）：收圈还没开始白底就上屏，
         放大时狐狸外面糊一圈白闪、缩小时 α 又赖在 1 不肯走（先闪一下才缩）；
         当时靠"把位移排到收圈之后、让 ramp 期 diskOff≈0"来回避，
         代价就是位移只能挤在最后 1/5 行程里 = 用户说的"缩完了才蹦一下"。 */
      const zS = sAvg * z2;
      const diskR = DISK_R * zS;
      const diskOff = zS * tp * F_OFF;
      const eWhite = K2(clamp((diskR + diskOff - R) / T.CLOSEUP_PAD_W, 0, 1));
      const padOn = eWhite > 0.004;
      if (padOn) {
        this._set(this.padCircle, "cx", ccx.toFixed(2));
        this._set(this.padCircle, "cy", ccy.toFixed(2));
        this._set(this.padCircle, "r", R.toFixed(2));
      }
      this._css(this.padCircle, "opacity", eWhite.toFixed(3));
      this._css(this.padCircle, "display", padOn ? "" : "none");
      /* 本体透明度（standby 呼吸淡出）——眼层要跟它同进同出，否则皮肤淡出时
         脸上还留着一块亮斑 */
      const bodyOp = (1 - (1 - ov.rX.tone) * ov.Lee) * (1 - ov.fade);
      /* 本体变换 = 锚点平移 ∘ 旋转 ∘ (本体尺度×z2) ∘ 支点回移。
         z2=1、tp=0 时与静止态逐字节一致（pivot=(360,360)）；tp=1 时不动点落在
         两眼中点 → 眼睛的屏幕位置 = 锚点（= 原版小点的位置）。 */
      const baseT =
        `translate(${(360 + tx).toFixed(2)} ${(360 + ty).toFixed(2)}) rotate(${rot.toFixed(3)}) scale(${(sx * z2).toFixed(4)} ${(sy * z2).toFixed(4)}) translate(${-pvx.toFixed(2)} ${-pvy.toFixed(2)})`;
      this._set(this.moverG, "transform", baseT);
      /* 眼层与本体层逐帧同参 → 眼睛永远长在正确的位置上 */
      this._set(this.eyeG, "transform", baseT);
      this._css(this.moverG, "opacity", bodyOp.toFixed(3));
      this._css(this.eyeG, "opacity", bodyOp.toFixed(3));
      /* 蓝盘主题（dotMode="blue"）：淡蓝实心圆与本体同变换（一起缩、一起摆），
         透明度只由缩点进度 yl 给 —— 不吃 bodyOp（那是给狐狸本体的色调衰减），
         否则"淡蓝色"会跟着变半透明。窗口放后半程（DOT_BLUE.fade），
         只对带 overlay 的产品态/Agent 形态生效（yl>0），idle 等恒为 0。 */
      if (this.dotG) {
        const eBlue = smoothstep(yl, ...this.dotBlue.fade);
        this._set(this.dotG, "transform", baseT);
        this._css(this.dotDisc, "opacity", eBlue.toFixed(3));
        this._css(this.dotDisc, "display", eBlue > 0.004 ? "" : "none");
      }
      /* 纯色盖板（index-eyes 皮肤）：最晚淡入——过早会把还在缩的蓝点整块盖白 */
      const fillOn = eFill > 0.004 && !!this.closeupFill;
      this._css(this.closeupFill, "opacity", eFill.toFixed(3));
      this._css(this.closeupFill, "display", fillOn ? "" : "none");
      /* 鼻子：特写态淡入（比盖板早一步收尾，免得盖板先压白、脸上却没有鼻子）。
         挂在脸坐标系里、但不跟视线走 → 与底图的鼻子逐帧重合，于是"狐狸头像消失"
         读作"缩成一张小狐狸脸"而不是"只剩两只眼睛"。
         主页面它正好压在底图鼻子之上（同位置同形状），肉眼无差别。 */
      if (this.noseEl) {
        const eNose = smoothstep(fo, ...T.CLOSEUP_FADE.nose);
        const noseOn = eNose > 0.004;
        this._css(this.noseEl, "opacity", eNose.toFixed(3));
        this._css(this.noseEl, "display", noseOn ? "" : "none");
      }

      /* 眼簇只随头倾斜，永不自转 */
      this.faceG.setAttribute(
        "transform",
        `translate(${T.PLACEMENT.mid[0]} ${T.PLACEMENT.mid[1]}) rotate(${T.PLACEMENT.theta})`
      );

      /* 视线合成（形态收敛时视线/指针幅度同步减小，防止眼睛移出取景圆） */
      const gw = (hasPtr ? 0.25 : 1) * (1 - fo * 0.8);
      const gp = 1 - fo * 0.8;
      const gx = this.gazeX.x * gw + this.pointer.x * gp;
      const gy = this.gazeY.x * gw + this.pointer.y * gp;
      this.gazeG.setAttribute(
        "transform",
        `translate(${gx.toFixed(2)} ${gy.toFixed(2)})`
      );

      /* 眼睛 */
      const morphT = clamp(this.morph.x, 0, 1);
      const polys = EY.livePolys(this.eyeFrom, this.eyeTo, morphT, this.liveFrom, {
        symMix: this.symMix,
        heart: this.heartMix,
      });
      /* 犯困：眼皮系数上再乘一个"睁↔闭进度"（nodClose=1 时眼睛几乎合死） */
      const nodLid = 1 - T.EYE_NOD.lid * this.nodClose;
      const lids = [
        EY.winkLid(this.blink.x, now, this.winkAt, this.winkEye, 0) * nodLid,
        EY.winkLid(this.blink.x, now, this.winkAt, this.winkEye, 1) * nodLid,
      ];
      const th = (-T.PLACEMENT.theta * Math.PI) / 180;
      const hg = hasPtr ? 0.45 : 0.3;
      const hlGx = gx * hg * Math.cos(th) - gy * hg * Math.sin(th);
      const hlGy = gx * hg * Math.sin(th) + gy * hg * Math.cos(th);
      /* 眼部整体缩放 = 状态基准 × 情绪增益（惊讶/困惑要"眼睛变大"） */
      if (ex.eyeBoost != null) this.eyeScale.t = ex.eyeBoost * this.fxBoost;
      else this.eyeScale.t = body.eyeBoost * this.fxBoost;
      const cross = T.LOOK_CROSS * this.crossMix;
      EY.paintEyes({
        els: this.eyeEls,
        hls: this.hlEls,
        hlHearts: this.hlHeartEls,
        decos: this.decos,
        polys,
        morphT,
        fromIdx: this.eyeFrom,
        toIdx: this.eyeTo,
        boost: this.eyeScale.x,
        lids,
        hlGx,
        hlGy,
        perEyeX: [cross, -cross],
        arc: { mix: this.arcMix, type: this.arcType || "smile" },
        heart: this.heartMix,
        orbit: this.orbitMix,
        hlHeart: this.hlHeartMix,
        phase: mt,
        glyph: {
          a: this.glyph.a,
          b: this.glyph.b,
          mix: this.glyph.mix,
          phase: mt,
        },
      });
      /* 眉毛已移除 —— 见 eyes.js / tables.js 说明 */

      /* overlay 渲染 + 哼歌双点 */
      this.fxLayer.paint(now, this.stateAt, this.ovKind, this.ovPrev, yl, mix, this.reduceMotion);
      const hum = clamp(this.humDots.x, 0, 1);
      if (hum > 0.01) {
        /* 音符双点：轨道半径/音符尺寸随本体尺度收缩——点形态下音符
           贴着小圆飘（≈1.3×点半径），全尺寸保持源版 1.3×体半径 */
        const noteK = Math.min(1, sAvg * 6);
        for (let i = 0; i < 2; i++) {
          const el = this.fxLayer.parts[3 + i];
          const Gn = this.ovSpin + i * Math.PI;
          const Ti = R0 * 1.3 * sAvg;
          const Ui = Math.cos(Gn);
          const Si = 0.55 + 0.45 * clamp((Ui + 1) / 2, 0, 1);
          el.style.display = "";
          el.setAttribute("cx", (R0 + Ti * Math.sin(Gn)).toFixed(1));
          el.setAttribute("cy", (R0 - Ti * 0.38 * Math.cos(Gn) - 8 * sAvg).toFixed(1));
          el.setAttribute("r", (7.5 * Si * hum * noteK).toFixed(2));
          el.setAttribute("opacity", ((0.3 + 0.7 * Si) * hum).toFixed(3));
        }
      }

      /* 粒子 */
      this.particles.update(now, this._dt ?? 0.016);
      /* 相机变焦：原版 ov.zoom verbatim（产品态点+特效的取景节奏） */
      if (Math.abs(ov.zoom - this.lastZoom) > 0.0001) {
        const half = 360 / ov.zoom;
        this.svg.setAttribute(
          "viewBox",
          `${(360 - half).toFixed(2)} ${(360 - half).toFixed(2)} ${(half * 2).toFixed(2)} ${(half * 2).toFixed(2)}`
        );
        this.lastZoom = ov.zoom;
      }

      this.onChange(this.snapshot());
    }
  }

  g.FoxAvatar = FoxAvatar;
})(window);
