import { PackageOpen } from "lucide-react";
import type { BaseLayoutProps } from "fumadocs-ui/layouts/shared";

export function baseOptions(): BaseLayoutProps {
  return {
    githubUrl: "https://github.com/aexhq/aex",
    nav: {
      title: (
        <span className="ant-nav-title">
          <span className="ant-logo-mark">ap</span>
          <span>aex</span>
        </span>
      )
    },
    links: [
      {
        icon: <PackageOpen />,
        text: "npm",
        url: "https://www.npmjs.com/package/@aexhq/sdk",
        active: "none",
        secondary: true
      }
    ]
  };
}
