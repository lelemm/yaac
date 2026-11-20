/** A minimal typed event source. */
export interface EventSource<T> {
  /** Subscribes; returns the unsubscribe function. */
  on(listener: (event: T) => void): () => void;
}

export class Emitter<T> implements EventSource<T> {
  private listeners = new Set<(event: T) => void>();

  on(listener: (event: T) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  emit(event: T): void {
    for (const l of [...this.listeners]) l(event);
  }
}
