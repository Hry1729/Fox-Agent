import { cloneElement, type ComponentProps, type ComponentType, type ReactElement } from 'react'
import { MessageCirclePlus as StaticMessageCirclePlus, Users as StaticUsers, Blocks as StaticBlocks, PanelLeft as StaticPanelLeft, PanelLeftOpen as StaticPanelLeftOpen, PanelRight as StaticPanelRight, Settings as StaticSettings, SunMoon as StaticSunMoon, Maximize as StaticMaximize, Minimize as StaticMinimize, Search as StaticSearch, Bell as StaticBell, Wallet as StaticWallet, Folders as StaticFolders, Library as StaticLibrary, Orbit as StaticOrbit, ListTodo as StaticListTodo, BookCopy as StaticBookCopy, House as StaticHouse, Sparkles as StaticSparkles, AlignLeft as StaticAlignLeft } from 'lucide-react'
import type { AnimateIconProps, IconProps } from '@/components/animate-ui/icons/icon'
import { deferredModule } from './use-deferred-module'

const implementation = deferredModule(() => import('./animated-icons-implementation'))
export type { IconProps, AnimateIconProps } from '@/components/animate-ui/icons/icon'
export type AnimatedIconTriggerProps = Omit<AnimateIconProps, 'asChild' | 'children' | 'loop' | 'animate' | 'animateOnView'> & { children: ReactElement }

function icon(name: 'MessageCirclePlus' | 'Users' | 'Blocks' | 'PanelLeft' | 'PanelLeftOpen' | 'PanelRight' | 'Settings' | 'SunMoon' | 'Maximize' | 'Minimize' | 'Search' | 'Bell' | 'Wallet' | 'Folders' | 'Library' | 'Orbit' | 'ListTodo' | 'BookCopy' | 'Home' | 'Sparkles' | 'AlignLeft', Fallback: typeof StaticBell) {
  return function DeferredIcon(props: IconProps<string>) {
    const loaded = implementation.useModule()
    if (loaded) { const Icon = loaded[name] as ComponentType<IconProps<string>>; return <Icon {...props} /> }
    // Motion-only props must never leak onto the fallback SVG.
    const { animate, animateOnHover, animateOnTap, animateOnView, animateOnViewMargin, animateOnViewOnce, animation, loop, loopDelay, initialOnAnimateEnd, completeOnStop, persistOnAnimateEnd, delay, ...svg } = props
    return <Fallback {...svg as ComponentProps<typeof StaticBell>} size={props.size ?? 28} />
  }
}
export const MessageCirclePlus = icon('MessageCirclePlus', StaticMessageCirclePlus)
export { MessageCirclePlus as MessageCirclePlusIcon }
export const Users = icon('Users', StaticUsers)
export { Users as UsersIcon }
export const Blocks = icon('Blocks', StaticBlocks)
export { Blocks as BlocksIcon }
export const PanelLeft = icon('PanelLeft', StaticPanelLeft)
export { PanelLeft as PanelLeftIcon }
export const PanelLeftOpen = icon('PanelLeftOpen', StaticPanelLeftOpen)
export { PanelLeftOpen as PanelLeftOpenIcon }
export const PanelRight = icon('PanelRight', StaticPanelRight)
export { PanelRight as PanelRightIcon }
export const Settings = icon('Settings', StaticSettings)
export { Settings as SettingsIcon }
export const SunMoon = icon('SunMoon', StaticSunMoon)
export { SunMoon as SunMoonIcon }
export const Maximize = icon('Maximize', StaticMaximize)
export { Maximize as MaximizeIcon }
export const Minimize = icon('Minimize', StaticMinimize)
export { Minimize as MinimizeIcon }
export const Search = icon('Search', StaticSearch)
export { Search as SearchIcon }
export const Bell = icon('Bell', StaticBell)
export { Bell as BellIcon }
export const Wallet = icon('Wallet', StaticWallet)
export { Wallet as WalletIcon }
export const Folders = icon('Folders', StaticFolders)
export { Folders as FoldersIcon }
export const Library = icon('Library', StaticLibrary)
export { Library as LibraryIcon }
export const Orbit = icon('Orbit', StaticOrbit)
export { Orbit as OrbitIcon }
export const ListTodo = icon('ListTodo', StaticListTodo)
export { ListTodo as ListTodoIcon }
export const BookCopy = icon('BookCopy', StaticBookCopy)
export { BookCopy as BookCopyIcon }
export const Home = icon('Home', StaticHouse)
export { Home as HomeIcon }
export const Sparkles = icon('Sparkles', StaticSparkles)
export { Sparkles as SparklesIcon }
export const AlignLeft = icon('AlignLeft', StaticAlignLeft)
export { AlignLeft as AlignLeftIcon }

export function AnimatedIconTrigger({ children, ...props }: AnimatedIconTriggerProps) {
  const loaded = implementation.useModule()
  if (loaded) return <loaded.AnimatedIconTrigger {...props}>{children}</loaded.AnimatedIconTrigger>
  const child = children as ReactElement<ComponentProps<'button'>>
  return cloneElement(child, {
    onPointerEnter: event => { implementation.preload(); child.props.onPointerEnter?.(event) },
    onFocus: event => { implementation.preload(); child.props.onFocus?.(event) },
  })
}
