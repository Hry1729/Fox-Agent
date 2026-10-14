// Source: https://animate-ui.com/r/primitives-base-switch.json
'use client';

import * as React from 'react';
import { Switch as SwitchPrimitives } from '@base-ui-components/react/switch';
import {
  motion,
  useReducedMotion,
  type TargetAndTransition,
  type VariantLabels,
  type HTMLMotionProps,
  type LegacyAnimationControls,
} from 'motion/react';

import { getStrictContext } from '@/lib/get-strict-context';
import { useControlledState } from '@/hooks/use-controlled-state';

type SwitchContextType = {
  isChecked: boolean;
  setIsChecked: SwitchProps['onCheckedChange'];
  isPressed: boolean;
  setIsPressed: (isPressed: boolean) => void;
};

const [SwitchProvider, useSwitch] =
  getStrictContext<SwitchContextType>('SwitchContext');

type SwitchProps = Omit<
  React.ComponentProps<typeof SwitchPrimitives.Root>,
  'render'
> &
  HTMLMotionProps<'button'> & { render?: React.ReactElement<Record<string, unknown>> };

function Switch({
  name,
  defaultChecked,
  checked,
  onCheckedChange,
  nativeButton = true,
  disabled,
  readOnly,
  required,
  inputRef,
  id,
  value,
  form,
  render,
  ...props
}: SwitchProps) {
  const [isPressed, setIsPressed] = React.useState(false);
  const [isChecked, setIsChecked] = useControlledState({
    value: checked,
    defaultValue: defaultChecked ?? false,
  });
  const handleCheckedChange: NonNullable<SwitchProps['onCheckedChange']> = (next, details) => {
    onCheckedChange?.(next, details);
    // Base lets consumers cancel a change before it becomes committed state.
    if (!details.isCanceled) setIsChecked(next);
  };

  // Radix's value/form props belong to the submitted checkbox, not just the visible button.
  const setInputRef = React.useCallback((node: HTMLInputElement | null) => {
    if (node) {
      node.value = String(value ?? 'on');
      if (form) node.setAttribute('form', form); else node.removeAttribute('form');
    }
    if (typeof inputRef === 'function') return inputRef(node);
    if (inputRef) inputRef.current = node;
  }, [inputRef, value, form]);

  return (
    <SwitchProvider
      value={{ isChecked, setIsChecked: handleCheckedChange, isPressed, setIsPressed }}
    >
      <SwitchPrimitives.Root
        data-slot="switch"
        data-state={isChecked ? "checked" : "unchecked"}
        name={name}
        checked={isChecked}
        onCheckedChange={handleCheckedChange}
        nativeButton={nativeButton}
        disabled={disabled}
        readOnly={readOnly}
        required={required}
        inputRef={setInputRef}
        id={id}
        render={render ? React.cloneElement(render, props) :
          <motion.button
            data-slot="switch"
            data-state={isChecked ? "checked" : "unchecked"}
            whileTap="tap"
            initial={false}
            onTapStart={() => setIsPressed(true)}
            onTapCancel={() => setIsPressed(false)}
            onTap={() => setIsPressed(false)}
            {...props}
          />
        }
      />
    </SwitchProvider>
  );
}

type SwitchThumbProps = Omit<
  React.ComponentProps<typeof SwitchPrimitives.Thumb>,
  'render'
> &
  HTMLMotionProps<'div'> & {
    pressedAnimation?:
      | TargetAndTransition
      | VariantLabels
      | boolean
      | LegacyAnimationControls;
  };

function SwitchThumb({
  pressedAnimation,
  transition = { type: 'spring', stiffness: 300, damping: 25 },
  ...props
}: SwitchThumbProps) {
  const { isPressed } = useSwitch();
  const reduced = useReducedMotion();

  return (
    <SwitchPrimitives.Thumb
      render={
        <motion.div
          data-slot="switch-thumb"
          layout={!reduced}
          transition={reduced ? { duration: 0 } : transition}
          animate={!reduced && isPressed ? pressedAnimation : undefined}
          {...props}
        />
      }
    />
  );
}

type SwitchIconPosition = 'left' | 'right' | 'thumb';

type SwitchIconProps = HTMLMotionProps<'div'> & {
  position: SwitchIconPosition;
};

function SwitchIcon({
  position,
  transition = { type: 'spring', bounce: 0 },
  ...props
}: SwitchIconProps) {
  const { isChecked } = useSwitch();
  const reduced = useReducedMotion();

  const isAnimated = React.useMemo(() => {
    if (position === 'right') return !isChecked;
    if (position === 'left') return isChecked;
    if (position === 'thumb') return true;
    return false;
  }, [position, isChecked]);

  return (
    <motion.div
      data-slot={`switch-${position}-icon`}
      animate={isAnimated ? { scale: 1, opacity: 1 } : { scale: 0, opacity: 0 }}
      transition={reduced ? { duration: 0 } : transition}
      {...props}
    />
  );
}

export {
  Switch,
  SwitchThumb,
  SwitchIcon,
  useSwitch,
  type SwitchProps,
  type SwitchThumbProps,
  type SwitchIconProps,
  type SwitchIconPosition,
  type SwitchContextType,
};
