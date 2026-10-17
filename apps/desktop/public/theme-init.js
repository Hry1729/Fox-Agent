// Run before the boot screen is painted; keep these keys aligned with color-theme and layoutStorage.
try {
  const theme = localStorage.getItem('fox.preferences.colorTheme')
  document.documentElement.dataset.colorTheme = ['graphite', 'paper', 'mist'].includes(theme) ? theme : 'graphite'
  document.documentElement.classList.toggle('dark', localStorage.getItem('fox.theme') === 'dark')
} catch {
  document.documentElement.dataset.colorTheme = 'graphite'
}
