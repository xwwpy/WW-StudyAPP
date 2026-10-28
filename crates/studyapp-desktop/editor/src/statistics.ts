export interface DocumentStatistics {
  words: number;
  characters: number;
  readingMinutes: number;
}

// Count the current Markdown source, excluding whitespace from character totals.
// Each Han character is one reading unit; other letter/number runs are words.
// Iterate CodeMirror chunks so large notes do not require a second text copy.
export function documentStatistics(chunks: Iterable<string>): DocumentStatistics {
  const steps = scanDocumentStatistics(chunks);
  let result = steps.next();
  while (!result.done) result = steps.next();
  return result.value;
}

// Yield within long lines as well as between CodeMirror chunks. String iteration
// advances by Unicode scalar, so a pause cannot split an astral character.
export function* scanDocumentStatistics(chunks: Iterable<string>): Generator<void, DocumentStatistics> {
  let words = 0, characters = 0, inWord = false;
  let remaining = 2048;
  for (const chunk of chunks) {
    for (const character of chunk) {
      const code = character.codePointAt(0)!;
      if (code <= 127) {
        if (code !== 32 && !(code >= 9 && code <= 13)) characters++;
        const letterOrNumber = code >= 48 && code <= 57 || code >= 65 && code <= 90 || code >= 97 && code <= 122;
        if (letterOrNumber && !inWord) words++;
        inWord = letterOrNumber || inWord && code === 39;
      } else {
        if (!/\s/u.test(character)) characters++;
        if (/\p{Script=Han}/u.test(character)) { words++; inWord = false; }
        else if (/[\p{L}\p{N}]/u.test(character)) { if (!inWord) words++; inWord = true; }
        else if (!/[\p{M}’]/u.test(character)) inWord = false;
      }
      if (--remaining === 0) { remaining = 2048; yield; }
    }
  }
  return { words, characters, readingMinutes: Math.max(1, Math.ceil(words / 400)) };
}
