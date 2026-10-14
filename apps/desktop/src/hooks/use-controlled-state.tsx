// Source: https://animate-ui.com/r/hooks-use-controlled-state.json
import * as React from 'react';
interface CommonControlledStateProps<T> { value?: T; defaultValue?: T; }
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function useControlledState<T, Rest extends any[] = []>(
  props: CommonControlledStateProps<T> & { onChange?: (value: T, ...args: Rest) => void },
): readonly [T, (next: T, ...args: Rest) => void] {
  const { value, defaultValue, onChange } = props;
  const isControlled = value !== undefined;
  const [state, setInternalState] = React.useState<T>(defaultValue as T);
  const setState = React.useCallback((next: T, ...args: Rest) => {
    // Controlled updates are requests: only the owner can accept a new value.
    if (!isControlled) setInternalState(next);
    onChange?.(next, ...args);
  }, [isControlled, onChange]);
  return [isControlled ? value : state, setState] as const;
}
