"use client";

import { usePathname } from "next/navigation";

/**
 * One navigation. On a narrow viewport it scrolls sideways inside its own strip;
 * the page body never does.
 */
export function WorkspaceNav({ slug, organizationSlug }: { slug: string; organizationSlug: string | null }) {
  const pathname = usePathname();
  const items: readonly { readonly href: string; readonly label: string }[] = [
    { href: `/w/${slug}/sessions`, label: "Sessions" },
    { href: `/w/${slug}/observability`, label: "Observability" },
    { href: `/w/${slug}/usage`, label: "Usage" },
    { href: `/w/${slug}/resources`, label: "Resources" },
    { href: `/w/${slug}/keys`, label: "API keys" },
    ...(organizationSlug ? [{ href: `/org/${organizationSlug}/billing`, label: "Billing" }] : []),
  ];
  return (
    <nav className="nav" aria-label="Workspace">
      <div className="frame nav-inner">
        {items.map((item) => (
          <a
            key={item.href}
            href={item.href}
            {...(pathname === item.href || pathname.startsWith(`${item.href}/`)
              ? { "aria-current": "page" as const }
              : {})}
          >
            {item.label}
          </a>
        ))}
      </div>
    </nav>
  );
}
