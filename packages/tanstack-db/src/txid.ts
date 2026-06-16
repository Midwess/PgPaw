const low32 = (txid: number): number => txid >>> 0

const MAX_SEEN = 4096

export class TxidTracker {
  private readonly seen = new Set<number>()
  private readonly waiters = new Set<() => void>()

  record(txid: number): void {
    this.seen.add(low32(txid))
    while (this.seen.size > MAX_SEEN) {
      const oldest = this.seen.values().next().value
      if (oldest === undefined) break
      this.seen.delete(oldest)
    }
    for (const notify of this.waiters) notify()
  }

  has(txid: number): boolean {
    return this.seen.has(low32(txid))
  }

  awaitTxId(txid: number, timeoutMs = 30000): Promise<void> {
    const key = low32(txid)
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
