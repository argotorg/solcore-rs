function markdownForContent(content) {
  if (typeof content === "string") {
    return { value: content };
  }

  if ("language" in content) {
    return { value: `\`\`\`${content.language}\n${content.value}\n\`\`\`` };
  }

  return { value: content.value };
}

export function markdownForHoverContents(contents) {
  const items = Array.isArray(contents) ? contents : [contents];
  return items.map(markdownForContent);
}
