export function stripFrontmatter(md: string): string {
  const normalized = md.replace(/^\uFEFF/, "");
  const frontmatter = normalized.match(/^---\r?\n[\s\S]*?\r?\n---\s*(?:\r?\n|$)/);
  return frontmatter ? normalized.slice(frontmatter[0].length).trimStart() : normalized;
}

export function prepareMarkdownForRender(md: string): string {
  return convertWikiLinks(stripFrontmatter(md));
}

export function polishRenderedMarkdown(html: string): string {
  return html
    .replace(/<li>\s*\[ \]\s*/g, '<li class="task-list-item"><span class="task-checkbox" aria-hidden="true"></span>')
    .replace(/<li>\s*\[[xX]\]\s*/g, '<li class="task-list-item task-list-item-done"><span class="task-checkbox checked" aria-hidden="true"></span>')
    .replace(/<li>\s*\[-\]\s*/g, '<li class="task-list-item task-list-item-paused"><span class="task-checkbox mixed" aria-hidden="true"></span>');
}

function convertWikiLinks(md: string): string {
  return md.replace(/(!?)\[\[([^\]\n]+)\]\]/g, (_match, bang: string, rawTarget: string) => {
    const [rawPath, rawAlias] = rawTarget.split("|");
    const target = rawPath.trim();
    const label = (rawAlias || target.split(/[\\/]/).pop() || target).trim();
    if (!target || !label) return _match;
    if (bang) return label;
    return `[${escapeMarkdownLinkText(label)}](#note:${encodeURIComponent(target)})`;
  });
}

function escapeMarkdownLinkText(value: string): string {
  return value.replace(/([\\\[\]])/g, "\\$1");
}
