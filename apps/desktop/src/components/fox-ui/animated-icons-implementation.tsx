import type { ReactElement } from 'react'
import { useReducedMotion } from 'motion/react'
import { AnimateIcon, type AnimateIconProps } from '@/components/animate-ui/icons/icon'

// Official Animate UI registry sources. Keep icons inside the trigger so the
// whole action (including its label) responds to hover / press, once per gesture.
export { MessageCirclePlus, MessageCirclePlus as MessageCirclePlusIcon } from '@/components/animate-ui/icons/message-circle-plus'
export { Users, Users as UsersIcon } from '@/components/animate-ui/icons/users'
export { Blocks, Blocks as BlocksIcon } from '@/components/animate-ui/icons/blocks'
export { PanelLeft, PanelLeft as PanelLeftIcon } from '@/components/animate-ui/icons/panel-left'
export { PanelRight, PanelRight as PanelRightIcon } from '@/components/animate-ui/icons/panel-right'
export { Settings, Settings as SettingsIcon } from '@/components/animate-ui/icons/settings'
export { SunMoon, SunMoon as SunMoonIcon } from '@/components/animate-ui/icons/sun-moon'
export { Maximize, Maximize as MaximizeIcon } from '@/components/animate-ui/icons/maximize'
export { Minimize, Minimize as MinimizeIcon } from '@/components/animate-ui/icons/minimize'
export { Search, Search as SearchIcon } from '@/components/animate-ui/icons/search'
export { Bell, Bell as BellIcon } from '@/components/animate-ui/icons/bell'
// Knowledge-sidebar / mode-switch set. Four are self-authored to the house
// contract because lucide-animated has no equivalent; the other two are ports of
// the registry components the user supplied (see each file's header).
export { Wallet, Wallet as WalletIcon } from '@/components/animate-ui/icons/wallet'
export { Folders, Folders as FoldersIcon } from '@/components/animate-ui/icons/folders'
export { Library, Library as LibraryIcon } from '@/components/animate-ui/icons/library'
export { Orbit, Orbit as OrbitIcon } from '@/components/animate-ui/icons/orbit'
export { ListTodo, ListTodo as ListTodoIcon } from '@/components/animate-ui/icons/list-todo'
export { BookCopy, BookCopy as BookCopyIcon } from '@/components/animate-ui/icons/book-copy'
export { Home, Home as HomeIcon } from '@/components/animate-ui/icons/home'
export { Sparkles, Sparkles as SparklesIcon } from '@/components/animate-ui/icons/sparkles'
export type { IconProps, AnimateIconProps } from '@/components/animate-ui/icons/icon'

export type AnimatedIconTriggerProps = Omit<AnimateIconProps, 'asChild' | 'children' | 'loop' | 'animate' | 'animateOnView'> & {
  children: ReactElement
}

/** Usage: <AnimatedIconTrigger><button ...><PanelRight /></button></AnimatedIconTrigger>. */
export function AnimatedIconTrigger({ children, animateOnHover = true, animateOnTap = true, ...props }: AnimatedIconTriggerProps) {
  const reduced = useReducedMotion()
  return <AnimateIcon {...props} asChild animate={false} animateOnHover={!reduced && animateOnHover} animateOnTap={!reduced && animateOnTap} loop={false} completeOnStop>{children}</AnimateIcon>
}
