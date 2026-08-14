import type { ReactElement } from "react";

import type { Paragraph } from "../_lib/marketing";

/**
 * Renders one parsed paragraph. The only inline form the content model has is
 * `code`, which becomes a real `<code>` element rather than styled text, so a
 * tool name is announced as code by a screen reader and is selectable as code.
 *
 * Spans are positional, immutable and never reordered, so their index is a
 * stable key.
 */
export function Inline({ paragraph }: Readonly<{ paragraph: Paragraph }>): ReactElement {
  return (
    <>
      {paragraph.map((span, position) =>
        span.kind === "code" ? (
          <code className="aex-code" key={`${position}-code`}>
            {span.value}
          </code>
        ) : (
          <span key={`${position}-text`}>{span.value}</span>
        )
      )}
    </>
  );
}
