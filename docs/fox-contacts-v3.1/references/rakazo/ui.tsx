export * from './upstream/dropdown-menu';
export * from './upstream/popover';
export * from './upstream/copy-text';
export * from './upstream/utils';
export const CopyTextFallback = () => null;
export const BotAvatar = () => null;
export const Button = ({ children, variant, size, ...props }: any) => <button {...props}>{children}</button>;
export const resolvePersonaColorDef = () => ({ hex: '#3EC5A8' });
