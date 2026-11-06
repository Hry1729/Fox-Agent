type Props = {
  base: string
  eyes: string
  fallbackFilter: string
  showFace: boolean
  hidden?: boolean
}

export function BlueFoxFallback({ base, eyes, fallbackFilter, showFace, hidden }: Props) {
  return <svg data-avatar-fallback="true" viewBox="0 0 720 720" style={{ display: hidden ? 'none' : undefined }}>
      <defs><filter id={fallbackFilter} colorInterpolationFilters="sRGB">
        <feColorMatrix in="SourceGraphic" type="matrix" values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 -5 0 5 0 -1" result="disk" />
        <feComposite in="disk" in2="SourceAlpha" operator="in" result="backdrop-mask" />
        <feFlood floodColor="var(--fox-avatar-color)" result="theme" />
        <feComposite in="theme" in2="backdrop-mask" operator="in" result="backdrop" />
        <feComposite in="SourceGraphic" in2="backdrop-mask" operator="out" result="fox" />
        <feComposite in="fox" in2="backdrop" operator="arithmetic" k2="1" k3="1" />
      </filter></defs>
      {showFace ? <>
        <image href={base} width="720" height="720" filter={`url(#${fallbackFilter})`} />
        <image href={eyes} width="720" height="720" />
      </> : <g fill="var(--fox-avatar-color)"><circle cx="200" cy="360" r="65" opacity=".65" /><circle cx="360" cy="360" r="65" /><circle cx="520" cy="360" r="65" opacity=".65" /></g>}
    </svg>
}
