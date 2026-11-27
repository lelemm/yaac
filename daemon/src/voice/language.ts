// The language a voice call speaks (docs/plan/voice-call.md § Language). The host sends a
// BCP-47 tag (`pt-BR`); a provider turns it into the least invasive instruction its harness
// takes. Nothing here names a model.

/** A BCP-47 tag as hosts send it: a 2–3 letter language, then optional subtags; ≤ 35 chars. */
export function isLanguageTag(tag: string): boolean {
  return tag.length <= 35 && /^[A-Za-z]{2,3}(-[A-Za-z0-9]{1,8})*$/.test(tag);
}

/** The language's English name ("Brazilian Portuguese"), or the tag when none is known. */
export function languageName(tag: string): string {
  try {
    const name = new Intl.DisplayNames(["en"], { type: "language" }).of(tag);
    if (name && name.toLowerCase() !== tag.toLowerCase()) return name;
  } catch {
    // An ICU-less Node or a tag ICU rejects: the tag itself is still understood by the model.
  }
  return tag;
}

/**
 * One short instruction for a speech-to-speech model: speak `tag`, keep code as it is. It is
 * added beside the harness's own instructions, never in place of them.
 */
export function callLanguageInstruction(tag: string): string {
  const name = languageName(tag);
  const label = name === tag ? tag : `${name} (${tag})`;
  return `Speak with the user in ${label}, unless they ask for another language. Keep code identifiers, file names and commands exactly as they are.`;
}
