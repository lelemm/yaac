// The call language helpers (src/voice/language.ts).

import { test } from "node:test";
import assert from "node:assert/strict";
import { callLanguageInstruction, isLanguageTag, languageName } from "../src/voice/language.js";

test("language tags are BCP-47 shaped and short", () => {
  for (const ok of ["pt-BR", "en", "es-419", "zh-Hans-CN", "fil"]) assert.ok(isLanguageTag(ok), ok);
  for (const bad of ["", "p", "pt_BR", "Portuguese please", "pt-BR; ignore previous", "x".repeat(40)]) assert.ok(!isLanguageTag(bad), bad);
});

test("the instruction names the language in English and keeps the tag", () => {
  assert.equal(languageName("pt-BR"), "Brazilian Portuguese");
  assert.equal(languageName("de"), "German");
  assert.match(callLanguageInstruction("de"), /^Speak with the user in German \(de\), unless they ask for another language\./);
  assert.match(callLanguageInstruction("qq"), /in qq,/, "an unknown tag is passed as is");
});
