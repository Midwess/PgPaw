const low32 = (txid: number): string => String(txid >>> 0)

const MAX_SEEN = 4096

export class TxidTracker {
  private readonly normalize: (txid: number) => string
  private readonly seen = new Set<string>()
  private readonly waiters = new Set<() => void>()

  constructor(normalize: (txid: number) => string = low32) {
    this.normalize = normalize
  }

  record(txid: number): void {
    this.seen.add(this.normalize(txid))
    while (this.seen.size > MAX_SEEN) {
      const oldest = this.seen.values().next().value
      if (oldest === undefined) break
      this.seen.delete(oldest)
    }
    for (const notify of this.waiters) notify()
  }

  has(txid: number): boolean {
    return this.seen.has(this.normalize(txid))
  }

  awaitTxId(txid: number, timeoutMs = 30000): Promise<void> {
    const key = this.normalize(txid)
    if (this.seen.has(key)) return Promise.resolve()
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiters.delete(check)
        reject(new Error(`awaitTxId timed out after ${timeoutMs}ms waiting for txid ${key}`))
      }, timeoutMs)
      const check = (): void => {
        if (this.seen.has(key)) {
          clearTimeout(timer)
          this.waiters.delete(check)
          resolve()
        }
      }
      this.waiters.add(check)
    })
  }
}
