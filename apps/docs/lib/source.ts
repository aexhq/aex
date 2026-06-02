import { createElement } from "react";
import {
  Blocks,
  BookOpenText,
  Box,
  Braces,
  KeyRound,
  Network,
  PackageCheck,
  Play,
  Route,
  ShieldCheck,
  TerminalSquare,
  Waves
} from "lucide-react";
import { docs } from "collections/server";
import { loader } from "fumadocs-core/source";

const icons = {
  Blocks,
  BookOpenText,
  Box,
  Braces,
  KeyRound,
  Network,
  PackageCheck,
  Play,
  Route,
  ShieldCheck,
  TerminalSquare,
  Waves
};

export const source = loader({
  source: docs.toFumadocsSource(),
  baseUrl: "/docs",
  url(slugs) {
    return slugs.length === 0 ? "/docs" : `/docs/${slugs.join("/")}`;
  },
  icon(icon) {
    if (!icon || !(icon in icons)) return;
    return createElement(icons[icon as keyof typeof icons]);
  }
});
