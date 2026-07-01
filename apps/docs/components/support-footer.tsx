import { Mail, MessageSquare } from "lucide-react";

/**
 * Sidebar footer rendered on every docs page via the shared DocsLayout.
 * Gives readers a support email and a feedback mailto from anywhere in the docs.
 */
export function SupportFooter() {
  return (
    <div className="flex flex-col gap-2 border-t border-fd-border pt-3 text-sm text-fd-muted-foreground">
      <a
        className="inline-flex items-center gap-2 transition-colors hover:text-fd-foreground"
        href="mailto:support@aex.dev"
      >
        <Mail className="size-4 shrink-0" aria-hidden="true" />
        <span>support@aex.dev</span>
      </a>
      <a
        className="inline-flex items-center gap-2 transition-colors hover:text-fd-foreground"
        href="mailto:support@aex.dev?subject=aex%20docs%20feedback"
      >
        <MessageSquare className="size-4 shrink-0" aria-hidden="true" />
        <span>Feedback</span>
      </a>
    </div>
  );
}
