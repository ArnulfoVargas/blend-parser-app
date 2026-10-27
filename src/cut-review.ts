/** Debounced, single-flight review. Obsolete replies can never approve a new plane. */
export class CutReview<T> {
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private waiting: { value: T; generation: number } | null = null;
  private running = false;
  constructor(private actions: { run(value: T): Promise<number>; pending(): void; result(objects: number | null, error?: unknown): void }, private delay = 450) {}

  schedule(value: T | null) {
    this.generation++;
    clearTimeout(this.timer);
    this.waiting = null;
    if (value === null) return;
    this.actions.pending();
    const generation = this.generation;
    this.timer = setTimeout(() => { this.waiting = { value, generation }; void this.flush(); }, this.delay);
  }

  private async flush() {
    if (this.running || !this.waiting) return;
    const request = this.waiting;
    this.waiting = null;
    this.running = true;
    try {
      const objects = await this.actions.run(request.value);
      if (request.generation === this.generation) this.actions.result(objects);
    } catch (error) {
      if (request.generation === this.generation) this.actions.result(null, error);
    } finally {
      this.running = false;
      void this.flush();
    }
  }
}
