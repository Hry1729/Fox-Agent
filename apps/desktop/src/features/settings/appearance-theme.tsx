import type { ReactNode } from 'react'
import { Check, Moon, Sun } from 'lucide-react'
import { cn } from '@/lib/utils'
import { COLOR_THEMES, type ColorTheme } from './color-theme'

/**
 * A miniature workbench painted with a theme's own tokens. The wrapper scopes
 * `data-color-theme` (and `dark`) to itself, so it shows the real palette of
 * that theme regardless of the one the app is currently using.
 */
export function ThemeSwatch({ theme, dark, className }: { theme: ColorTheme; dark: boolean; className?: string }) {
  return (
    <span
      aria-hidden="true"
      data-color-theme={theme}
      className={cn(dark && 'dark', 'relative flex h-[72px] overflow-hidden rounded-[var(--fox-radius-control)] border border-border bg-background', className)}
    >
      <span className="flex w-[30%] flex-col gap-1 border-r border-sidebar-border bg-sidebar p-1.5">
        <span className="h-1.5 w-3/4 rounded-full bg-sidebar-accent" />
        <span className="h-1.5 w-1/2 rounded-full bg-muted-foreground/25" />
        <span className="h-1.5 w-2/3 rounded-full bg-muted-foreground/25" />
      </span>
      <span className="flex flex-1 flex-col gap-1.5 p-2">
        <span className="ml-auto h-2.5 w-1/2 rounded-[var(--fox-radius-small)] bg-user-bubble" />
        <span className="h-1.5 w-5/6 rounded-full bg-muted-foreground/30" />
        <span className="h-1.5 w-2/3 rounded-full bg-muted-foreground/30" />
        <span className="mt-auto flex items-center justify-end gap-1">
          <span className="h-3 w-6 rounded-[var(--fox-radius-small)] border border-border bg-card" />
          <span className="h-3 w-8 rounded-[var(--fox-radius-small)] bg-primary" />
        </span>
      </span>
    </span>
  )
}

function ThemeOption({ selected, onSelect, swatch, label, description, icon }: {
  selected: boolean
  onSelect: () => void
  swatch: ReactNode
  label: string
  description?: string
  icon?: ReactNode
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      onClick={onSelect}
      className={cn(
        'relative flex flex-col gap-2 rounded-[var(--fox-radius-card)] border bg-card p-1.5 text-left transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/40',
        selected ? 'border-brand' : 'border-border hover:border-foreground/25',
      )}
    >
      {swatch}
      <span className="flex items-start gap-1.5 px-1 pb-1">
        {icon ? <span className="mt-0.5 text-muted-foreground [&_svg]:size-3.5">{icon}</span> : null}
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="text-[length:var(--fox-type-chrome)]! leading-5 font-medium text-foreground">{label}</span>
          {description ? <span className="text-[length:var(--fox-type-caption)]! leading-4 text-muted-foreground">{description}</span> : null}
        </span>
      </span>
      {selected ? <span aria-hidden="true" className="absolute top-2.5 right-2.5 grid size-4 place-items-center rounded-full bg-brand text-brand-foreground shadow-[var(--fox-shadow-sm)]"><Check className="size-2.5" strokeWidth={3} /></span> : null}
    </button>
  )
}

function OptionGroup({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="mb-4 flex flex-col gap-2">
      <span className="text-[length:var(--fox-type-caption)]! font-medium text-muted-foreground">{label}</span>
      <div role="radiogroup" aria-label={label} className="grid grid-cols-3 gap-2">{children}</div>
    </div>
  )
}

export function AppearanceThemeControls({ dark, onDarkChange, colorTheme, onColorThemeChange }: {
  dark: boolean
  onDarkChange: (dark: boolean) => void
  colorTheme: ColorTheme
  onColorThemeChange: (theme: ColorTheme) => void
}) {
  return (
    <>
      <OptionGroup label="明暗模式">
        <ThemeOption selected={!dark} onSelect={() => onDarkChange(false)} icon={<Sun />} label="浅色" swatch={<ThemeSwatch theme={colorTheme} dark={false} />} />
        <ThemeOption selected={dark} onSelect={() => onDarkChange(true)} icon={<Moon />} label="深色" swatch={<ThemeSwatch theme={colorTheme} dark />} />
      </OptionGroup>
      <OptionGroup label="配色主题">
        {COLOR_THEMES.map((theme) => (
          <ThemeOption
            key={theme.id}
            selected={colorTheme === theme.id}
            onSelect={() => onColorThemeChange(theme.id)}
            label={theme.label}
            description={theme.description}
            swatch={<ThemeSwatch theme={theme.id} dark={dark} />}
          />
        ))}
      </OptionGroup>
    </>
  )
}
