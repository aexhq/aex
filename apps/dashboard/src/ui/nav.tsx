"use client";

import { usePathname } from "next/navigation";

/**
 * One navigation. On a narrow viewport it scrolls sideways inside its own strip;
 * the page body never does.
 */
export function WorkspaceNav({ slug }: { slug: string }) {
  const pathname = usePathname();
  const items: readonly { readonly href: string; readonly label: string }[] = [
    { href: `/w/${slug}/sessions`, label: "Sessions" },
    { href: `/w/${slug}/files`, label: "Files" },
    { href: `/w/${slug}/keys`, label: "API keys" },
    { href: "/billing", label: "Billing" },
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
