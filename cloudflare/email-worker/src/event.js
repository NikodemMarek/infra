const SNIPPET_LENGTH = 500;

const ENTITIES = { amp: "&", lt: "<", gt: ">", quot: '"', nbsp: " " };

export function kindOf(envelopeTo) {
  const at = envelopeTo.lastIndexOf("@");
  const local = at < 0 ? envelopeTo : envelopeTo.slice(0, at);
  return local.toLowerCase() === "hook" ? "hook" : "throwaway";
}

function decodeEntities(text) {
  return text.replace(/&(amp|lt|gt|quot|nbsp|#\d+);/g, (match, name) => {
    if (name[0] !== "#") return ENTITIES[name];
    const code = Number(name.slice(1));
    return code <= 0x10ffff ? String.fromCodePoint(code) : match;
  });
}

function htmlToText(html) {
  return decodeEntities(
    html
      .replace(/<(style|script)\b[^>]*>[\s\S]*?<\/\1\s*>/gi, " ")
      .replace(/<[^>]*>/g, " "),
  );
}

function snippetOf(parsed) {
  const source = parsed.text || (parsed.html ? htmlToText(parsed.html) : "");
  return source.replace(/\s+/g, " ").trim().slice(0, SNIPPET_LENGTH);
}

function fromOf(parsed, envelopeFrom) {
  const { name, address } = parsed.from ?? {};
  if (!address) return envelopeFrom;
  return name ? `${name} <${address}>` : address;
}

function toOf(kind, envelopeTo, parsed) {
  if (kind === "throwaway") return envelopeTo;
  return parsed.to?.find((entry) => entry.address)?.address ?? envelopeTo;
}

export function toEvent(kind, envelopeTo, envelopeFrom, parsed) {
  return {
    kind,
    from: fromOf(parsed, envelopeFrom),
    to: toOf(kind, envelopeTo, parsed),
    subject: parsed.subject ?? "",
    snippet: snippetOf(parsed),
  };
}
