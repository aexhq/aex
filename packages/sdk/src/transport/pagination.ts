import { AexStreamProtocolError } from "./errors.js";

export class Page<T> implements AsyncIterable<T> {
  readonly items: readonly T[];
  readonly nextCursor: string | undefined;
  readonly #next: ((cursor: string) => Promise<Page<T>>) | undefined;

  constructor(items: readonly T[], nextCursor?: string, next?: (cursor: string) => Promise<Page<T>>) {
    this.items = items;
    this.nextCursor = nextCursor;
    this.#next = next;
  }

  async *pages(): AsyncGenerator<Page<T>> {
    const seen = new Set<string>();
    let page: Page<T> | undefined = this;
    while (page) {
      yield page;
      const cursor = page.nextCursor;
      if (!cursor) return;
      if (seen.has(cursor)) throw new AexStreamProtocolError(`repeated cursor ${cursor}`);
      seen.add(cursor);
      if (!page.#next) throw new AexStreamProtocolError("page has a cursor but no continuation");
      page = await page.#next(cursor);
    }
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<T> {
    for await (const page of this.pages()) {
      for (const item of page.items) yield item;
    }
  }
}
