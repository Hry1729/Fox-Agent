(function () {
  'use strict';

  var root = document.documentElement;
  var STORE_THEME = 'foxds.theme';
  var STORE_MODE = 'foxds.mode';
  var THEME_NAMES = { graphite: '石墨', paper: '暖纸', mist: '雾蓝' };

  var themeButtons = Array.prototype.slice.call(document.querySelectorAll('[data-set-theme]'));
  var themeName = document.getElementById('theme-name');
  var modeToggle = document.getElementById('mode-toggle');
  var modeLabel = document.getElementById('mode-label');
  var contrastTheme = document.getElementById('contrast-theme');

  function read(key) {
    try {
      return window.localStorage.getItem(key);
    } catch (error) {
      return null;
    }
  }

  function write(key, value) {
    try {
      window.localStorage.setItem(key, value);
    } catch (error) {
      /* storage may be unavailable; the page still works for this session */
    }
  }

  function currentTheme() {
    var value = root.getAttribute('data-color-theme');
    return value === 'paper' || value === 'mist' ? value : 'graphite';
  }

  function currentMode() {
    return root.classList.contains('dark') ? 'dark' : 'light';
  }

  function updateLabels() {
    var theme = currentTheme();
    var dark = currentMode() === 'dark';

    themeButtons.forEach(function (button) {
      button.setAttribute('aria-pressed', String(button.getAttribute('data-set-theme') === theme));
    });

    if (themeName) themeName.textContent = THEME_NAMES[theme];
    if (modeToggle) modeToggle.setAttribute('aria-pressed', String(dark));
    if (modeLabel) modeLabel.textContent = dark ? '切换浅色' : '切换深色';
    if (contrastTheme) contrastTheme.textContent = THEME_NAMES[theme] + ' · ' + (dark ? '深色' : '浅色');
  }

  function renderContrast() {
    var rows = Array.prototype.slice.call(document.querySelectorAll('#contrast-body tr'));
    if (!rows.length) return;

    rows.forEach(function (row) {
      var fg = resolveColor(row.getAttribute('data-fg'));
      var bg = resolveColor(row.getAttribute('data-bg'));
      var ratio = contrastRatio(toRgb(fg), toRgb(bg));
      var min = parseFloat(row.getAttribute('data-min')) || 4.5;
      var ratioCell = row.querySelector('.ratio');
      var verdict = row.querySelector('.verdict');

      if (!ratioCell || !verdict) return;

      if (!isFinite(ratio)) {
        ratioCell.textContent = '—';
        verdict.className = 'verdict';
        verdict.textContent = '—';
        return;
      }

      ratioCell.textContent = ratio.toFixed(2) + ':1';
      var pass = ratio >= min;
      verdict.className = 'verdict ' + (pass ? 'pass' : 'fail');
      verdict.textContent = pass ? '通过' : '不足';
      verdict.setAttribute('title', '要求 ≥ ' + min + ':1');
    });
  }

  function resolveColor(variable) {
    var probe = document.createElement('span');
    probe.style.cssText = 'position:absolute;left:-9999px;top:0;width:1px;height:1px;color:var(' + variable + ')';
    document.body.appendChild(probe);
    var color = window.getComputedStyle(probe).color;
    probe.remove();
    return color;
  }

  var canvas = document.createElement('canvas');
  canvas.width = 1;
  canvas.height = 1;
  var context = canvas.getContext('2d', { willReadFrequently: true });

  function toRgb(cssColor) {
    if (!context) return [0, 0, 0];
    context.clearRect(0, 0, 1, 1);
    context.fillStyle = '#000000';
    context.fillStyle = cssColor;
    context.fillRect(0, 0, 1, 1);
    var data = context.getImageData(0, 0, 1, 1).data;
    return [data[0], data[1], data[2]];
  }

  function channel(value) {
    var c = value / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  }

  function luminance(rgb) {
    return 0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2]);
  }

  function contrastRatio(a, b) {
    var l1 = luminance(a);
    var l2 = luminance(b);
    var light = Math.max(l1, l2);
    var dark = Math.min(l1, l2);
    return (light + 0.05) / (dark + 0.05);
  }

  themeButtons.forEach(function (button) {
    button.addEventListener('click', function () {
      var theme = button.getAttribute('data-set-theme');
      root.setAttribute('data-color-theme', theme);
      write(STORE_THEME, theme);
      updateLabels();
      renderContrast();
    });
  });

  if (modeToggle) {
    modeToggle.addEventListener('click', function () {
      var dark = currentMode() !== 'dark';
      root.classList.toggle('dark', dark);
      write(STORE_MODE, dark ? 'dark' : 'light');
      updateLabels();
      renderContrast();
    });
  }

  /* Scroll spy ------------------------------------------------------------ */

  var navLinks = Array.prototype.slice.call(document.querySelectorAll('.rail-nav a'));
  var sections = navLinks
    .map(function (link) {
      var id = link.getAttribute('href');
      return id && id.charAt(0) === '#' ? document.querySelector(id) : null;
    })
    .filter(Boolean);

  var ticking = false;

  function updateActiveSection() {
    ticking = false;
    if (!sections.length) return;

    var marker = 140;
    var active = sections[0];

    sections.forEach(function (section) {
      if (section.getBoundingClientRect().top <= marker) active = section;
    });

    navLinks.forEach(function (link) {
      var match = link.getAttribute('href') === '#' + active.id;
      link.classList.toggle('is-active', match);
      if (match) {
        link.setAttribute('aria-current', 'true');
      } else {
        link.removeAttribute('aria-current');
      }
    });
  }

  function requestActiveUpdate() {
    if (ticking) return;
    ticking = true;
    window.requestAnimationFrame(updateActiveSection);
  }

  window.addEventListener('scroll', requestActiveUpdate, { passive: true });
  window.addEventListener('resize', requestActiveUpdate);

  /* Text scale ------------------------------------------------------------ */

  var scaleRange = document.getElementById('scale-range');
  var scaleOutput = document.getElementById('scale-output');
  var scaleDemo = document.getElementById('scale-demo');

  if (scaleRange && scaleOutput && scaleDemo) {
    scaleRange.addEventListener('input', function () {
      var value = Number(scaleRange.value);
      var scale = value / 100;
      scaleDemo.style.setProperty('--fox-text-scale', String(scale));
      scaleOutput.textContent = value + '%';
    });
  }

  /* Motion replay --------------------------------------------------------- */

  var stage = document.getElementById('motion-stage');
  var replay = document.getElementById('replay-motion');

  if (stage && replay) {
    replay.addEventListener('click', function () {
      stage.classList.remove('is-playing');
      /* force a reflow so the animation can restart */
      void stage.offsetWidth;
      stage.classList.add('is-playing');
      window.setTimeout(function () {
        stage.classList.remove('is-playing');
      }, 900);
    });
  }

  updateLabels();
  renderContrast();
  updateActiveSection();
})();
