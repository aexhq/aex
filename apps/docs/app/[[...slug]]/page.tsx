import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { DocsBody, DocsDescription, DocsPage, DocsTitle } from "fumadocs-ui/layouts/docs/page";
import { HomePage } from "@/components/home";
import { getMDXComponents } from "@/components/mdx";
import { source } from "@/lib/source";

type PageProps = {
  params: Promise<{
    slug?: string[];
  }>;
};

export function generateStaticParams() {
  return source.generateParams();
}

export async function generateMetadata({ params }: PageProps): Promise<Metadata> {
  const { slug } = await params;
  const page = source.getPage(slug ?? []);

  if (!page) return {};

  if ((slug ?? []).length === 0) {
    return {
      title: "antpath",
      description: page.data.description
    };
  }

  return {
    title: page.data.title,
    description: page.data.description
  };
}

export default async function Page({ params }: PageProps) {
  const { slug } = await params;
  const slugs = slug ?? [];
  const page = source.getPage(slugs);

  if (!page) notFound();

  const MDX = page.data.body;
  const isHome = slugs.length === 0;

  return (
    <DocsPage full={isHome} toc={isHome ? [] : page.data.toc}>
      {isHome ? null : (
        <>
          <DocsTitle>{page.data.title}</DocsTitle>
          <DocsDescription>{page.data.description}</DocsDescription>
        </>
      )}
      <DocsBody>
        {isHome ? <HomePage /> : <MDX components={getMDXComponents()} />}
      </DocsBody>
    </DocsPage>
  );
}
