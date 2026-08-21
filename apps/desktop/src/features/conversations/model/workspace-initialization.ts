export const WORKSPACE_INITIALIZATION_TIMEOUT_MS = 8_000

export function withWorkspaceInitializationTimeout<T>(
  operation: Promise<T>,
  label: string,
  timeoutMs = WORKSPACE_INITIALIZATION_TIMEOUT_MS,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`${label}超时，请重试`))
    }, timeoutMs)

    operation.then(
      (value) => {
        clearTimeout(timer)
        resolve(value)
      },
      (error) => {
        clearTimeout(timer)
        reject(error)
      },
    )
  })
}
