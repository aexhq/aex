export async function parseNdjsonFrames(chunks: AsyncIterable<string> | Iterable<string>): Promise<unknown[]> {
  const frames: unknown[] = [];
  let buffered = "";
  for await (const chunk of chunks) {
    buffered += chunk;
    for (;;) {
      const newline = buffered.indexOf("\n");
      if (newline < 0) break;
      const line = buffered.slice(0, newline).replace(/\r$/, "");
      buffered = buffered.slice(newline + 1);
      if (line.trim()) frames.push(JSON.parse(line));
    }
  }
  return frames;
}
