/* Fox port — L4: calibrated placement + full 39-state tables.
   Placement constants measured from Fox头像.png (720x720 space, y-down).
   EYE_PLAYLIST / EYE_HOLD_MS / BLINK_MS / GROUPS copied from Grok tables.js. */
(function (g) {
  const { rand, sign } = g.GROK_MATH;

  const PLACEMENT = {
    mid: [277.5, 544.5],   // 两眼连线中点（原图实测值，与图片眼位对齐）
    theta: 23.8,           // 眼轴角 = 头部主轴 PCA 实测 27.9° 与原版眼线 23.8° 取一致（deg，顺时针）
    eyeDX: 132.75,         // 半眼距（沿眼轴）
    eyeR: 61,              // idle 眼形目标长半轴（px），对齐原版狐狸眼尺寸
    maxExt: 76,            // 单个眼形渲染半径上限（防个别 Grok 大眼形盖脸）
    sqY: 0.85,             // 纵向压缩（先于椭圆化，决定最终 bbox）
    minRatio: 0.78,        // 竖直眼形最终目标宽高比（不足则加宽到该值）；0.85 太圆 → 0.78 略细长
    ellipseK: 1.0,         // 椭圆化强度：1 = 竖直眼形完全替换为同 bbox 的标准椭圆（原版狐狸就是纯椭圆）
    maxGrow: 1.08,         // 尺寸钳制：任何眼形最终 maxR ≤ idle 眼 × 1.08（抑制"双眼放大变圆"）
    closedTh: 62,          // 局部 y 范围低于此值视为眯眼弧线（4/13/22）→ 隐藏高光（sqY 后眯眼 ≈41、睁眼 ≈96）
    hlR: [9.9, 9.6],       // 高光点半径（实测；位置由各眼形几何动态计算）
    eyeFill: "#17181b",
    hlFill: "#ffffff",
    /* 鼻子（PIL 实测 fox-base.png：黑像素 bbox 85×59、质心 (240.9,606.6)、
       长轴倾角 20°；π·rx·ry 反算面积 3939 vs 实测 3860 → 是椭圆不是圆角矩形）。
       换算到「脸局部系」（原点 = 两眼中点 mid、坐标轴随 theta 转过）后 =
       (u −8.4, v +71.6)、倾角 −3.8°（几乎与眼轴平行地躺在眼睛下方）。
       挂进 eyeG 的独立组（与眼睛同坐标系、同变换，但不跟视线平移）→ 与脸严格等比。
       只有皮肤声明 faceNose 才建（index-eyes.html：纯色盖板把底图鼻子盖掉了）。
       主页面不建：底图自带鼻子，而它并非完美椭圆（径向 26.2~45.8），矢量副本
       要么越界留 1px 接缝、要么内切缩水 11%，不如不画。 */
    nose: { at: [240.9, 606.6], rx: 42.5, ry: 29.5, rot: 20.0 },
    noseFill: "#171b2b",   // 实测鼻心 rgba(23,27,43)：比眼黑略偏蓝
  };

  const GROUPS = [
    { label: "生命周期", states: ["sleeping", "waking", "idle", "listening", "thinking", "searching"] },
    { label: "情绪反应", states: ["excited", "surprised", "suspicious", "angry", "drowsy", "happy", "curious", "confused", "bored", "proud", "shy", "sad", "playful", "celebrate"] },
    { label: "Agent 形态", states: ["orbit", "radar", "progress"] },
    { label: "产品动作", states: ["spawning", "humming", "loading", "dictating", "writing", "sending", "receiving", "uploading", "alerting", "dragging", "bouncing", "powering-down"] },
  ];
  const STATES = GROUPS.flatMap((x) => x.states);

  /* 怼脸/产品形态（单层连续变形，见 fox.js _paint）：产品态里本体按原版 wre 语义
     缩成小点，同时追加两件连续量（同一个标量 fo = min(cu, yl) 驱动）：
       ① 缩放支点从画布中心 (360,360) 滑到两眼中点 CLOSEUP_AT —— 眼睛最终正好
          落在画布锚点上（= 原版小点的位置）；观感就是"狐狸一边往右上挪、
          一边被镜头拉近"。（旧版把焦点钉在取景圆心上 → 圆盘跟着脸一起被拽到
          图片下方 → 产品动作的点整体偏下 ~30px。）
       ② 内容放大 1+fo×CLOSEUP_ZOOM，取景圆半径 512→352（乘本体尺度）收到
          原版蓝点大小 —— 圈里露出的正是脸部白色区域（眼睛），蓝圈/耳朵出画。
     关键：取景圆半径大于圆盘轮廓时裁剪是空操作，此时绝不能铺白，否则狐狸外面
     多出一圈白 = "白色闪一下"。白垫底的上屏判据由几何量给出（fox.js eWhite）。 */
  const CLOSEUP = new Set(GROUPS[3].states);
  const CLOSEUP_BODY = 0.5; // 已废弃：本体尺度回归原版 wre 语义，仅保留导出兼容
  const CLOSEUP_AT = [277.5, 544.5];
  const CLOSEUP_ZOOM = 0.565;
  /* 包络（自变量 = 形态进度 fo）：
       ZOOM_IN 内容放大 —— 起得晚：前半程保持原版"缩点"形态，后半程才把蓝圈/
                         耳朵推出取景圆、露出脸部白毛。
       SHIFT   位移曲线 —— 全程：位移就是"狐狸一边缩、一边往右上挪"本身。
                         旧版排到 fo 0.50 之后才起步，观感是"都已经缩成最小圆了
                         才突然蹦一下"（实测：R 在前 14 帧就从 511 掉到 129，
                         而位移全程为 0，第 15 帧起才在 25 帧里挪完）。
                         敢提前的依据：白垫底 α 已由几何量驱动（fox.js eWhite，
                         自变量正是"露白宽度"），露白从 0 连续长出来时 α 才同步
                         爬升，不再有"该露白处从深色瞬变白"的跳变。所以位移提前
                         只会让露白更早出现一点点，不会闪。
       FACE    纯色盖板（皮肤项 closeupFill，index-eyes.html 用）—— 完成得最晚。 */
  const CLOSEUP_ZOOM_IN = [0.28, 1.00];
  /* 位移曲线（自变量 = fo）：g(fo) 单调 0→1，就是"狐狸往右上挪"的进度本身。
     偏心量 d = 202 × 终态尺度 × g 由它直接给出（fox.js 据此反解支点进度），
     与本体缩点彻底解耦 —— 于是位移能从第一帧起步、且全程单调。
     对比：直接写 tp = smoothstep(fo) 时 d = Sx×202×tp，被 Sx 牵着走 ——
     早期 Sx≈1 而 tp≈0 → 位移≈0（"都缩成小圆了才突然蹦一下"）；
     想把 tp 提前，d 又会在中途冲过终态值（先飘出去再弹回来）。 */
  const CLOSEUP_SHIFT = [0.05, 0.85];
  /* 标定常数：终态本体尺度 = 原版 wre 最小缩点 × 特写放大 (1+CLOSEUP_ZOOM)。
     它必须出现在公式里 —— 因为"眼睛最终落在锚点上"这个几何约束本身就等价于
     d = 202 × 终态尺度。改 wre 或 CLOSEUP_ZOOM 时同步更新。 */
  const CLOSEUP_SHIFT_TO = 0.26;
  /* 白垫底 α 的过渡宽度（root/720 空间 px）：α 只在这段"露白宽度"内爬升，之后
     恒为 1。窗口越窄，"深色月牙 → 白月牙"的可见过渡量越小。 */
  const CLOSEUP_PAD_W = 8;
  /* 鼻子淡入：与纯色盖板同起点、但收尾更早（盖板 0.60→0.90，鼻子 0.60→0.75）。
     两个约束夹出来的：
       ① 不能早于盖板 —— 盖板之前底图的鼻子还看得见，这时叠一个矢量鼻子（底图鼻子
          不是完美椭圆）会在鼻缘拉一圈 1px 接缝；
       ② 必须早于盖板过半 —— 盖板把底图鼻子压白的同时，矢量鼻子得已经画满，否则
          中间会有一瞬"鼻子被洗掉又回来"。
     主页面不含 faceNose，所以只有纯眼页看得到这条曲线。 */
  const CLOSEUP_FADE = { face: [0.60, 0.90], nose: [0.60, 0.75] };
  /* 取景圆半径系数插值端点（乘以本体尺度 sAvg）。
     起点从 512 收到 430：露白宽度 = R + d − ρ，而 512 时 R 比圆盘轮廓 ρ 大出
     150×sAvg —— 这段纯"富余"在位移提前后会被直接算进露白，早期就摊出一块偏宽
     的白月牙。430 仍罩得住画布内容（耳朵外缘 380 < 430，fo≲0.15 之前裁不到
     耳朵），但把富余砍掉约一半 → 露白峰值同步回落。
     收满 352 = 圆盘半径 = 原版蓝点尺寸。 */
  const CLOSEUP_FRAME = [430, 352];

  /* —— 蓝盘主题（index-blue.html）——
     整块皮肤**只有一个颜色：淡蓝 #8FC6FE**。缩点收满后是一枚淡蓝实心圆，
     overlay 周围的小圆（跳动点 / 环绕粒子 / 圈 / 字形）也全是同一枚淡蓝，
     连底盘（assets/fox-base-blue.png）都是这个色 —— 缩放全程无跳色。
     数值都是量的，不是选的：
       r     = 底盘半径。狐狸轮廓（含耳朵尖）相对画布中心的最大半径实测
               361.33 < 362 → 圆盘刚好完整罩住整只狐狸，"缩点后是个纯圆"
               而不是"圆里还有一撮毛"。
       fill  = 淡蓝，和 assets/fox-base-blue.png 里那枚烘焙底盘**逐通道相等**
               （那枚盘子就是把原图 rgb(33,143,253) 的蓝盘换成这个色做的），
               所以缩点读起来是"狐狸化进盘里"，而不是"盖了一块别的蓝"。
       fade  = 缩点进度 yl 上的淡入窗口。放后半程：头像先照常缩小，蓝再漫上来，
               收满时已是实心圆（早于 0.55 就上色会把还在缩的整只狐狸罩成色块）。 */
  const DOT_BLUE = {
    r: 362,
    fill: "#8FC6FE",
    fade: [0.55, 0.95],
  };
  /* overlay 周围元素（跳动点 / 环绕粒子 / 圈 / 字形）统一淡蓝。
     两格**同色**是刻意的：OverlayLayer 内部按 i%2 在 [色1, 色2] 间交替，
     给成同色就等于关掉交替 → 整圈元素与缩点后的实心圆完全同色，
     视觉上高度一致（想要"淡蓝 + 白"的双色观感，把第二格改回 "#FFFFFF"）。 */
  const ORN_BLUE = ["#8FC6FE", "#8FC6FE"];
  /* 撒花粒子：同样只有淡蓝一色（原为六色彩虹，在单一色系里最跳）。 */
  const ORN_BURST = ["#8FC6FE"];

  /* 源表 verbatim（u3 眼形索引）。
     情绪反应组的 14 条已重排：原表大量共用同一组合（surprised 与 curious 都是
     [3,21]、confused 与 suspicious 都是 [14,5]、drowsy/sad/bored 挤在 [4,22…]、
     celebrate/playful/proud 挤在 [2,8,17…]），这正是"14 个情绪一半重复"的根因。
     重排依据是对 25 套眼形的实地目视分类：
       2/8/10/11/17/19/20/21 = 大椭圆（睁眼）   7/16 = 收紧中椭圆
       6/15/24 = 小眼                            4/13/22 = 眯缝横线
       5/14/23 = 半闭（黑椭圆 + 眼睑横线）       9/18 = 圆睁环（惊讶/缩瞳感）
       3/12 = 带视线偏移（上抬 / 斜视）          0/1 = idle 基准（带高光）
     现在 14 个情绪两两之间最多只共用一个索引，且组合互不相同。 */
  const EYE_PLAYLIST = {
    sleeping: [13, 22, 4], waking: [13], idle: [0, 8], listening: [10, 1, 19],
    thinking: [8, 16, 14, 17, 5], searching: [15, 9, 3, 20, 12, 18],
    excited: [2, 21, 11, 17],           // 大椭圆高频快切（配 spark + 抖视）
    surprised: [8, 2],                  // 标准竖椭圆 87×111（18/3 归一化后是横的，放大会"变宽"）
    suspicious: [14, 12],               // 半闭 + 斜视偏移
    angry: [16, 7],                     // 收紧中椭圆（怒视）
    drowsy: [0, 13],                    // 醒眼 0 ↔ 睡眼 13：由 nod 驱动来回切换，不走定时播放
    happy: [2, 19],                     // 弧线笑眼接管，本体只是过渡底
    curious: [3, 8],                    // 上抬 + 大椭圆（边扫视边睁大）
    confused: [2, 8],                   // 标准竖椭圆（配"高光点绕圈"）
    bored: [4, 13],                     // 眯缝横线（配上翻视线）
    proud: [20, 8],                     // 大椭圆微抬
    /* 害羞＝正常大小的大眼（配爱心高光）。原先用 6/15 号"小眼"（仅标准眼的 85%）
       + eyeBoost 0.90 → 实测眼睛只有 idle 的 77%，看起来"眼睛缩了"。
       7/11 都是 87.2×111.5 的标准竖椭圆，与所有其它情绪"最多共用一个索引"。 */
    shy: [7, 11],
    sad: [24, 6],                       // 小眼（配泪滴）
    playful: [21, 11],                  // 大椭圆快切（配星星眼）
    celebrate: [10, 20],                // 圆睁欢呼（18/9 是全表最不对称的素材：一眼 111×98 一眼 62×57，必大小眼）
    orbit: [0, 8], radar: [0, 8], progress: [0, 8],
    spawning: [3, 0], humming: [0, 8], loading: [0, 8], dictating: [10, 1, 19],
    sending: [0, 8], receiving: [19, 0, 8], uploading: [15, 9, 8],     writing: [15, 9],
    alerting: [3, 21], bouncing: [2, 17],
    dragging: [3, 15, 0], "powering-down": [13, 22],
  };

  const EYE_HOLD_MS = {
    sleeping: [6000, 10000], waking: [800, 800], idle: [9000, 16000],
    listening: [2800, 5000],     thinking: [2000, 3600], searching: [1000, 1800],
    excited: [1100, 2000], surprised: [2500, 4000],
    suspicious: [2600, 4500], angry: [2200, 3800], drowsy: [4000, 8000],
    happy: [2500, 4500], curious: [1800, 3200], confused: [2200, 3800],
    bored: [3500, 6000], proud: [3500, 6000], shy: [3000, 5500],
    sad: [4000, 7000],
    playful: [1500, 3000], celebrate: [1400, 2600], orbit: [4000, 8000],
    radar: [4000, 8000], progress: [4000, 8000], spawning: [1200, 1200],
    humming: [5000, 9000], loading: [6000, 10000], dictating: [4000, 8000],
    sending: [4000, 8000], receiving: [4000, 8000], uploading: [4000, 8000],
    writing: [4000, 8000], alerting: [2000, 3600],
    bouncing: [3000, 6000], dragging: [1600, 3000], "powering-down": [6000, 9000],
  };

  const BLINK_MS = {
    sleeping: null, waking: null, idle: [6000, 14000], listening: [3000, 7000],
    thinking: [3500, 7000], searching: [1600, 4000],
    excited: [2000, 4000], surprised: [1800, 3500], suspicious: [4500, 8000],
    angry: [3500, 7000], drowsy: null, happy: [2500, 5000], curious: [2500, 5500],
    confused: [2800, 5500], bored: [4000, 8000], proud: [3500, 7000],
    shy: [3000, 6000], sad: [4000, 8000],
    playful: [2000, 4500], celebrate: [2200, 4500],
    orbit: null, radar: null, progress: null, spawning: null, humming: [4000, 8000],
    loading: null, dictating: null, sending: null, receiving: null, uploading: null,
    writing: null, alerting: null, bouncing: null,
    dragging: [2200, 4500], "powering-down": null,
  };

  /* 身体（整个头像容器）参数 per state
     breath [amp, Hz] 呼吸缩放 | ty [amp, Hz, bias] 上下浮动(px)
     tilt 常驻倾角(deg) | roll [amp, Hz] 摇摆(deg)
     eyeBoost 眼睛放大系数 | lidBase 常驻眼皮(1=全睁)
     autoHop [min,max]s 自动跳 | stiff 眼 morph 弹簧刚度 */
  const BODY = {
    "sleeping":       { breath: [0.010, 0.22], ty: [2, 0.18, 4],   tilt: -3, roll: [0.4, 0.10], eyeBoost: 0.90, lidBase: 0.10, autoHop: null,          stiff: 5 },
    "waking":         { breath: [0.012, 0.90], ty: [3, 0.90, 2],   tilt: 0,  roll: [1.5, 0.70], eyeBoost: 1.05, lidBase: 1,    autoHop: null,          stiff: 9 },
    idle:             { breath: [0.008, 0.45], ty: [2, 0.5, 0],    tilt: 0,  roll: [1.2, 0.30], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    listening:        { breath: [0.007, 0.50], ty: [1.5, 0.5, 0],  tilt: -4, roll: [1.0, 0.35], eyeBoost: 1.05, lidBase: 1,    autoHop: null,          stiff: 6 },
    thinking:         { breath: [0.007, 0.42], ty: [1.5, 0.4, -1], tilt: 3,  roll: [0.8, 0.25], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    searching:        { breath: [0.009, 0.90], ty: [2, 1.0, 0],    tilt: 4,  roll: [1.8, 0.90], eyeBoost: 1.06, lidBase: 1,    autoHop: null,          stiff: 10 },
    excited:          { breath: [0.014, 1.60], ty: [4, 1.4, -2],   tilt: 0,  roll: [2.5, 1.30], eyeBoost: 1.05, lidBase: 1,    autoHop: [3500, 7000],  stiff: 10 },
    surprised:        { breath: [0.004, 0.30], ty: [0.6, 0.3, 0],  tilt: 0,  roll: [0.4, 0.15], eyeBoost: 1.10, lidBase: 1,    autoHop: null,          stiff: 8 },
    suspicious:       { breath: [0.005, 0.35], ty: [1, 0.35, 0],   tilt: -6, roll: [0.5, 0.20], eyeBoost: 0.98, lidBase: 0.82, autoHop: null,          stiff: 6 },
    angry:            { breath: [0.011, 1.10], ty: [2, 1.0, -1],   tilt: 0,  roll: [1.5, 1.00], eyeBoost: 1.05, lidBase: 0.90, autoHop: null,          stiff: 9 },
    drowsy:           { breath: [0.011, 0.26], ty: [2, 0.22, 3],   tilt: -4, roll: [0.5, 0.12], eyeBoost: 0.90, lidBase: 0.50, autoHop: null,          stiff: 5 },
    happy:            { breath: [0.010, 0.80], ty: [2.5, 0.8, 0],  tilt: 0,  roll: [2.2, 0.90], eyeBoost: 1.04, lidBase: 1,    autoHop: [9000, 16000], stiff: 6 },
    curious:          { breath: [0.008, 0.50], ty: [1.5, 0.5, 0],  tilt: 6,  roll: [1.0, 0.40], eyeBoost: 1.06, lidBase: 1,    autoHop: null,          stiff: 6 },
    confused:         { breath: [0.007, 0.50], ty: [2, 0.6, 0],    tilt: -7, roll: [1.2, 0.40], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    bored:            { breath: [0.006, 0.25], ty: [2, 0.25, 2],   tilt: -4, roll: [0.4, 0.12], eyeBoost: 0.92, lidBase: 0.70, autoHop: null,          stiff: 5 },
    proud:            { breath: [0.010, 0.70], ty: [2.5, 0.7, 1],  tilt: 5,  roll: [1.5, 0.60], eyeBoost: 1.08, lidBase: 1,    autoHop: [9000, 16000], stiff: 7 },
    shy:              { breath: [0.009, 0.60], ty: [2, 0.6, 2],    tilt: -8, roll: [0.8, 0.30], eyeBoost: 1,    lidBase: 0.75, autoHop: null,          stiff: 6 },
    sad:              { breath: [0.006, 0.30], ty: [1.5, 0.3, 4],  tilt: -5, roll: [0.5, 0.15], eyeBoost: 0.95, lidBase: 0.85, autoHop: null,          stiff: 5 },
    playful:          { breath: [0.011, 1.10], ty: [3, 1.1, 0],    tilt: 0,  roll: [2.5, 1.10], eyeBoost: 1.05, lidBase: 1,    autoHop: [6000, 12000], stiff: 8 },
    celebrate:        { breath: [0.014, 1.50], ty: [4, 1.5, -2],   tilt: 0,  roll: [2.5, 1.30], eyeBoost: 1.10, lidBase: 1,    autoHop: null,          stiff: 10 },
    orbit:            { breath: [0.006, 0.40], ty: [1, 0.4, 0],    tilt: 0,  roll: [0.5, 0.20], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    radar:            { breath: [0.006, 0.40], ty: [1, 0.4, 0],    tilt: 0,  roll: [0.5, 0.20], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    progress:         { breath: [0.006, 0.40], ty: [1, 0.4, 0],    tilt: 0,  roll: [0.5, 0.20], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    spawning:         { breath: [0.012, 1.20], ty: [2, 1.0, 0],    tilt: 0,  roll: [1.0, 0.80], eyeBoost: 1.05, lidBase: 1,    autoHop: null,          stiff: 8 },
    humming:          { breath: [0.010, 0.90], ty: [3, 0.9, 0],    tilt: 0,  roll: [2.0, 0.80], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    loading:          { breath: [0.006, 0.50], ty: [1, 0.5, 0],    tilt: 0,  roll: [0.5, 0.20], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    dictating:        { breath: [0.008, 0.80], ty: [2, 0.8, 0],    tilt: -3, roll: [1.0, 0.50], eyeBoost: 1.03, lidBase: 1,    autoHop: null,          stiff: 6 },
    writing:          { breath: [0.007, 0.60], ty: [1.5, 0.6, -1], tilt: 4,  roll: [0.8, 0.30], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    sending:          { breath: [0.006, 0.50], ty: [1, 0.5, 0],    tilt: 0,  roll: [0.5, 0.20], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    receiving:        { breath: [0.006, 0.50], ty: [1, 0.5, 0],    tilt: 0,  roll: [0.5, 0.20], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    uploading:        { breath: [0.006, 0.50], ty: [1, 0.5, 0],    tilt: 0,  roll: [0.5, 0.20], eyeBoost: 1,    lidBase: 1,    autoHop: null,          stiff: 6 },
    alerting:         { breath: [0.005, 1.80], ty: [1.5, 1.4, 0],  tilt: 0,  roll: [1.0, 1.40], eyeBoost: 1.06, lidBase: 1,    autoHop: null,          stiff: 9 },
    dragging:         { breath: [0.008, 1.40], ty: [1, 1.2, 2],    tilt: 8,  roll: [2.0, 1.20], eyeBoost: 1.08, lidBase: 1,    autoHop: null,          stiff: 8 },
    bouncing:         { breath: [0.012, 1.50], ty: [3, 1.5, -2],   tilt: 0,  roll: [1.5, 1.10], eyeBoost: 1.06, lidBase: 1,    autoHop: [1600, 3000],  stiff: 9 },
    "powering-down":  { breath: [0.008, 0.30], ty: [1.5, 0.3, 3],  tilt: -3, roll: [0.4, 0.12], eyeBoost: 0.95, lidBase: 0.70, autoHop: null,          stiff: 5 },
  };

  /* 无指针时的视线游移目标（720 空间 px），节奏仿 Grok nextGaze */
  const GAZE = {
    sleeping:  () => ({ x: rand(-0.1, 0.1) * 20, y: rand(0.3, 0.6) * 12, hold: [5000, 9000] }),
    waking:    () => ({ x: rand(-0.3, 0.3) * 20, y: rand(-0.3, 0.1) * 12, hold: [1500, 3000] }),
    idle:      () => ({ x: rand(-0.3, 0.3) * 20, y: rand(-0.3, 0.3) * 12, hold: [2500, 5500] }),
    listening: () => ({ x: rand(-0.3, 0.3) * 20, y: rand(-0.25, 0.25) * 12, hold: [2200, 4200] }),
    thinking:  () => ({ x: sign() * rand(0.5, 1) * 20, y: -rand(0.4, 1) * 12, hold: [1500, 2800] }),
    searching: () => ({ x: rand(-1, 1) * 20, y: rand(-0.8, 0.4) * 12, hold: [600, 1300] }),
    excited:   () => ({ x: rand(-1, 1) * 20, y: rand(-1, 0.3) * 12, hold: [700, 1400] }),
    surprised: () => ({ x: 0, y: -8, hold: [2200, 3600] }),
    suspicious:() => ({ x: sign() * rand(0.3, 0.6) * 20, y: rand(-0.1, 0.2) * 12, hold: [2600, 4600] }),
    angry:     () => ({ x: sign() * rand(0.2, 0.5) * 20, y: rand(0, 0.4) * 12, hold: [1800, 3400] }),
    drowsy:    () => ({ x: rand(-0.2, 0.2) * 20, y: rand(0.2, 0.6) * 12, hold: [4000, 7000] }),
    happy:     () => ({ x: rand(-0.5, 0.5) * 20, y: rand(-0.4, 0.2) * 12, hold: [1600, 3000] }),
    curious:   () => ({ x: sign() * rand(0.4, 1) * 20, y: rand(-0.6, 0.2) * 12, hold: [1200, 2400] }),
    confused:  () => ({ x: rand(-0.6, 0.6) * 20, y: rand(-0.5, 0.3) * 12, hold: [1100, 2200] }),
    bored:     () => ({ x: rand(-0.15, 0.15) * 20, y: rand(0.1, 0.4) * 12, hold: [4000, 7000] }),
    proud:     () => ({ x: rand(-0.2, 0.2) * 20, y: -rand(0.2, 0.5) * 12, hold: [2400, 4200] }),
    shy:       () => ({ x: rand(-0.2, 0.2) * 20, y: rand(0.2, 0.5) * 12, hold: [2400, 4200] }),
    sad:       () => ({ x: rand(-0.25, 0.25) * 20, y: rand(0.3, 0.7) * 12, hold: [3500, 6000] }),
    playful:   () => ({ x: rand(-0.7, 0.7) * 20, y: rand(-0.5, 0.4) * 12, hold: [900, 1800] }),
    celebrate: () => ({ x: rand(-0.5, 0.5) * 20, y: -rand(0.3, 0.7) * 12, hold: [900, 1800] }),
    orbit:     () => ({ x: Math.sin(performance.now() * 0.0011) * 14, y: rand(-0.1, 0.1) * 12, hold: [300, 500] }),
    radar:     () => ({ x: rand(-0.1, 0.1) * 20, y: rand(-0.1, 0.1) * 12, hold: [2600, 4200] }),
    progress:  () => ({ x: rand(-0.15, 0.15) * 20, y: -rand(0.1, 0.3) * 12, hold: [2600, 4200] }),
    spawning:  () => ({ x: rand(-0.3, 0.3) * 20, y: rand(-0.2, 0.3) * 12, hold: [900, 1800] }),
    humming:   () => ({ x: rand(-0.4, 0.4) * 20, y: rand(-0.3, 0.2) * 12, hold: [1800, 3400] }),
    loading:   () => ({ x: rand(-0.1, 0.1) * 20, y: rand(-0.1, 0.1) * 12, hold: [3000, 5000] }),
    dictating: () => ({ x: rand(-0.2, 0.2) * 20, y: rand(-0.1, 0.3) * 12, hold: [2000, 3800] }),
    writing:   () => ({ x: rand(-0.4, -0.1) * 20 * sign(), y: rand(0.1, 0.4) * 12, hold: [1400, 2600] }),
    sending:   () => ({ x: rand(0.3, 0.6) * 20 * sign(), y: rand(-0.5, -0.2) * 12, hold: [1600, 3000] }),
    receiving: () => ({ x: rand(-0.6, -0.3) * 20 * sign(), y: rand(-0.5, -0.2) * 12, hold: [1600, 3000] }),
    uploading: () => ({ x: rand(-0.2, 0.2) * 20, y: -rand(0.3, 0.6) * 12, hold: [2000, 3600] }),
    alerting:  () => ({ x: 0, y: -10, hold: [1400, 2600] }),
    dragging:  () => ({ x: sign() * rand(0.4, 0.8) * 20, y: rand(-0.2, 0.4) * 12, hold: [900, 1800] }),
    bouncing:  () => ({ x: rand(-0.5, 0.5) * 20, y: rand(-0.4, 0.4) * 12, hold: [1000, 2000] }),
    "powering-down": () => ({ x: rand(-0.1, 0.1) * 20, y: rand(0.1, 0.4) * 12, hold: [3600, 6400] }),
  };

  /* 源 verbatim：wink 状态、特技菜单状态（V=转圈/弹跳, B=花式） */
  const WINK_STATES = new Set(["idle", "happy", "excited", "curious", "playful"]);
  const V_T = new Set(["happy", "excited", "proud"]);
  const B_T = new Set(["playful"]);

  /* ============================ 情绪差异化（眼部语言） ============================
     背景：25 套眼形的几何区分度其实很有限 —— 除少数几类（圆睁环 9/18、眯缝线 4/13/22、
     半闭 5/14/23、小眼 6/15/24、带视线偏移的 3/12），其余都是一大堆近似的竖椭圆。
     只靠"填哪几个索引"做情绪，结果必然是一半重复。
     所以差异化拆成两个正交维度，14 个情绪各拿一套唯一组合：
       ① look  视线行为 —— 眼睛"怎么看"：扫视 / 斜睨 / 上翻 / 躲闪 / 斗鸡眼 / 绕圈
       ② glyph 眼内装饰 —— 星星眼、爱心眼、泪滴、晕圈、螺旋、缩瞳环
       两者之外的第三维是 arc（笑眼弧线）：直接接管眼形，画出 ^ ^ 弯月。
     未在 EYE_FX 里声明的状态（生命周期 / Agent 形态 / 产品动作）一律保持原版：
     glyph 不画、arc 不画、视线走原 GAZE 表 —— 完全零行为变化。
     注：眉毛（BROW_PRESETS）与 brow 维度曾作为第一信号存在，后按需求整体移除
     —— 眉毛只该长在头骨上，而这里只有眼睛和鼻子，多出来的两道弧反而抢戏。 */

  /* 视线行为：[hold] 型每段停顿后换目标；连续型（sweep / circle）由 fox.js 按相位
     逐帧取值。幅度单位与 GAZE 同（脸局部系 px）。 */
  const LOOK = {
    stare:  () => ({ x: 0, y: 0, hold: [900, 1500] }),          // 锁定凝视
    side:   () => ({ x: sign() * 21, y: -2, hold: [2400, 3800] }), // 斜睨一边不动
    roll:   () => ({ x: 0, y: -16, hold: [1500, 2600] }),        // 上翻（翻白眼）
    dodge:  () => ({ x: rand(-14, 14), y: 9, hold: [900, 1700] }), // 躲闪下垂
    down:   () => ({ x: rand(-6, 6), y: 13, hold: [2200, 3400] }), // 垂视
    up:     () => ({ x: rand(-8, 8), y: -13, hold: [2000, 3200] }),// 仰视
    wander: () => ({ x: rand(-9, 9), y: rand(-5, 6), hold: [3600, 5600] }), // 慢漂
    cross:  () => ({ x: 0, y: 1, hold: [2000, 3200] }),          // 斗鸡眼（内聚另加）
    dart:   () => ({ x: rand(-11, 11), y: rand(-7, 4), hold: [170, 380] }), // 高频抖动
  };
  /* 连续视线：x = ax·sin/cos(2πf·t)，y = by（sweep）或 ay·sin(2πf·t)（circle） */
  const LOOK_CONT = {
    sweep:  { ax: 25, ay: 0, by: -4, f: 0.40 },   // 好奇：左右来回扫
    circle: { ax: 19, ay: 9, by: 0,  f: 0.42 },   // 贪玩：绕小圈转
  };
  /* 斗鸡眼的内聚量（每眼朝鼻侧的偏移，px）。只对 look === "cross" 生效 */
  const LOOK_CROSS = 12;

  /* 情绪 → 眼部语言。字段：
       look  视线行为（可选）
       glyph 眼内装饰（可选，取 GLYPH_KINDS 之一）
       arc   笑眼弧线（可选，"smile"）
       shape 眼形本体接管（可选，"heart" = 眼睛直接长成一颗心，多边形逐点形变）
       sym   true = 强制左右眼同形等大（源数据里 3/5/9/12/14/18/21/23 号眼形
             本来就是"一只大一只小/一只睁一只眯"的配对，选中它们就会出现"一大一小"）
       boost 眼部额外放大增益（1 = 不动；最终 = BODY.eyeBoost × boost）
       nod   true = 眼睛按"点头打瞌睡"的节奏在睁 / 闭之间来回切换（犯困，见 EYE_NOD）
       orbit true = 高光白点沿眼内椭圆轨道绕圈（困惑）
       hlHeart true = 高光白点换成白色爱心（害羞）
     14 条组合两两不同 —— 不再有两个情绪共享同一套眼睛。
     高光白点：glyph / arc / shape 出现时会把高光白点淡出（装饰自己就是眼睛的内容，
     再叠一颗白点会变成"星星上糊了个白圈"）；没有装饰的情绪照常保留高光。 */
  /* 高光点绕圈（困惑）：白点沿眼形内的椭圆轨道匀速打转。f = 每秒圈数；
     rx / ry = 轨道半轴（眼形 bbox 半宽 / 半高的倍数）——取值保证"白点贴到
     轨道最外沿也不越出眼形"（rx·半宽 + hlR < 半宽）。 */
  const EYE_ORBIT = { f: 0.62, rx: 0.56, ry: 0.53 };

  /* 害羞：高光白点 → 白色爱心（EYE_FX 的 hlHeart）。
     size = 心高 / **参照眼高**（标准竖椭圆眼 87.2×111.5 的高度，见 eyes.js 的
     HEART_REF_EYE_H）—— 注意是参照眼高，不是"当前这套眼形的高度"：
     爱心尺寸只跟随整体缩放（boost × 呼吸 × 眨眼压扁），
     于是把害羞的眼睛从小眼换回正常大眼时，爱心的**像素尺寸保持不变**。
     cy = 心中心相对眼中心的偏移，单位 = 当前眼形半高（负 = 上）。
     0.32 是按改前的爱心像素高 ≈35.8px 反推的（原来是小眼 79.6px × 0.45）。 */
  const EYE_HL_HEART = { size: 0.32, cy: -0.45 };

  /* 犯困（nod）：眼睛在"睁（苏醒）"与"闭（睡着）"之间来回切换 —— 像点头打瞌睡。
     一个周期按相位 u ∈ [0,1) 分四段：
       [0, awake)              睁着
       [awake, closeEnd)       慢慢沉下去（smoothstep 0→1）—— 越困合得越慢
       [closeEnd, shutEnd)     闭住
       [shutEnd, 1)            迅速弹开（smoothstep 1→0）—— 就是打瞌睡被惊醒那一下
     产出的 drop ∈ [0,1]（0 = 全醒、1 = 睡死）同时驱动两件事：
       ① 眼皮系数 1 − lid·drop（乘在眨眼系数上）→ 眼睛真的合上；
       ② 眼形 morph 目标：drop 跨过 flip 时在 openIdx（睁眼 0 号）与 shutIdx（睡着 13 号）
          之间切换一次 —— 闭眼走"慢慢沉"的低刚度、睁眼走"一下弹开"的高刚度。
     两件事合起来才是"睡着"（源 sleeping 态正是 13 号眼形 + 眼皮几乎合死）。
     lid 取 0.55 而不是 0.9：眼睛闭上时形状 13 本来就只剩 26% 高（103.4 → 26.5 单位），
     再压到 0.10 会变成一条 2.6 单位的发丝 —— 头像一缩小就"眼睛不见了"（正是开心那处
     要修的毛病）。0.55 让闭眼停在 ~11.6 单位（≈ 睡着观感的那条线），既读得出"闭着"
     又不会消失。 */
  const EYE_NOD = {
    period: 4.6, awake: 0.20, closeEnd: 0.66, shutEnd: 0.80, flip: 0.5,
    lid: 0.55, openIdx: 0, shutIdx: 13, droopStiff: 2.2, wakeStiff: 7.5,
  };

  /* 笑眼衔接（进 / 出开心）：眼睛"先变扁、再变弯"。
     原先进 happy 时眼形按 arcMix 直接淡出、弧线同时淡入 —— 0.1 秒内整只眼睛就没了，
     读起来是"眼睛突然消失"。现在把这件事拆成四段，自变量统一取 arcMix：
       flatT   把眼睛压扁成一条线（最早，就是犯困那种"眯成一条缝"）
       riseT   弧线从"压扁线"长高成弯月（中）
       arcT    弧线淡入（中）
       bodyT   压扁的眼形本体让位给弧线（最晚）
     ⇒ 观感：睁眼 → 眯成一条线 → 线往上弯成 ^^。反向离开时自动逆序（先变直、再回圆）。
     stiff 比其它表情量低（4.5 vs 7）—— 整个衔接约 0.6s，慢到看得清"先扁后弯"。
     bodyT 收在 0.85 就结束：弧线在 arcMix≈0.85 时已基本长满，压扁的那条线再留在
     半透明状态只会和弧线叠成一团灰（实测 arcMix 0.75~0.95 会看到"线 + 弧"双影）。 */
  const EYE_ARC = {
    squash: 0.16,
    flatT: [0.00, 0.62],
    riseT: [0.50, 0.92],
    arcT: [0.46, 0.82],
    bodyT: [0.55, 0.85],
    stiff: 4.5,
  };

  const EYE_FX = {
    excited:   { look: "dart",   glyph: "spark" },
    /* 惊讶＝整体等比睁大：boost 只乘缩放、不改形状，且眼形必须选"标准竖椭圆"
       —— 18 / 9 / 12 号归一化后宽 111.8 > 高（1.14 / 1.28），放大后就是"变宽"。 */
    surprised: { look: "stare",  sym: true, boost: 1.26 },
    suspicious:{ look: "side",   glyph: null },
    angry:     { look: "stare",  glyph: null },
    /* 犯困＝眼睛在"睁（苏醒 0 号）"与"闭（睡着 13 号）"之间慢速来回切换（nod）：
       睁住 → 慢慢沉下去 → 闭住 → 迅速弹开，像点头打瞌睡。
       旧的"半闭 + 慢漂"跟狐疑的"半闭 + 斜睨"读起来几乎一样，这才是"犯困≈狐疑"的根因。 */
    drowsy:    { look: "wander", glyph: null, nod: true },
    happy:     { look: "wander", glyph: null, arc: "smile" },
    curious:   { look: "sweep",  glyph: null },
    /* 困惑＝大眼 + 斗鸡眼 + 高光白点在眼里转圈（不用蚊香圈：一根静止的螺旋读不出"晕"） */
    confused:  { look: "cross",  sym: true, boost: 1.30, orbit: true },
    bored:     { look: "roll",   glyph: null },
    proud:     { look: "up",     glyph: null },
    /* 害羞＝眼形照旧（小眼 + 躲闪视线），只把高光白点换成白色爱心 */
    shy:       { look: "dodge",  hlHeart: true },
    sad:       { look: "down",   glyph: "tear" },
    playful:   { look: "circle", glyph: "star" },
    celebrate: { look: "up",     glyph: "spark" },
  };
  /* 眼部语言的淡入淡出刚度（弹簧）：比眨眼慢、比换眼形快 —— 看上去像"表情收放" */
  const EYE_FX_STIFF = 7;

  const SPRINGS = {
    morph: [7, 1],
    blink: [26, 1],
    eyeScale: [9, 0.85],
    gazeX: [13, 1],
    gazeY: [13, 1],
    squash: [10, 0.8],
    spinTurn: [6.2, 1],
    overlay: [14, 1],
    overlayMix: [11, 1],
    closeup: [7, 1],
  };

  const LABELS = {
    sleeping: "睡着", waking: "苏醒", idle: "待机", listening: "聆听",
    thinking: "思考", searching: "搜索",
    excited: "兴奋", surprised: "惊讶", suspicious: "狐疑", angry: "生气",
    drowsy: "犯困", happy: "开心", curious: "好奇", confused: "困惑",
    bored: "无聊", proud: "骄傲", shy: "害羞", sad: "难过",
    playful: "贪玩", celebrate: "庆祝",
    orbit: "环绕", radar: "雷达", progress: "进度",
    spawning: "生成", humming: "哼歌", loading: "加载", dictating: "语音",
    writing: "书写", sending: "发送", receiving: "接收", uploading: "上传",
    alerting: "警报", dragging: "拖拽", bouncing: "弹跳",
    "powering-down": "关机",
  };

  g.FOX_TABLES = {
    PLACEMENT, GROUPS, STATES, EYE_PLAYLIST, EYE_HOLD_MS, BLINK_MS,
    BODY, GAZE, WINK_STATES, V_T, B_T, SPRINGS, LABELS,
    LOOK, LOOK_CONT, LOOK_CROSS, EYE_FX, EYE_FX_STIFF, EYE_ORBIT, EYE_HL_HEART,
    EYE_NOD, EYE_ARC,
    CLOSEUP, CLOSEUP_BODY, CLOSEUP_AT, CLOSEUP_ZOOM,
    CLOSEUP_FADE, CLOSEUP_FRAME, CLOSEUP_ZOOM_IN, CLOSEUP_SHIFT, CLOSEUP_SHIFT_TO, CLOSEUP_PAD_W,
    DOT_BLUE, ORN_BLUE, ORN_BURST,
  };
})(window);
