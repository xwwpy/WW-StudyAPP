import { Text } from "@codemirror/state";

export function scalarText(value: string): boolean {
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code >= 0xD800 && code <= 0xDBFF) {
      const next = value.charCodeAt(++index);
      if (!(next >= 0xDC00 && next <= 0xDFFF)) return false;
    } else if (code >= 0xDC00 && code <= 0xDFFF) return false;
  }
  return true;
}

export function splitsScalar(text: Text, position: number): boolean {
  if (position <= 0 || position >= text.length) return false;
  const pair = text.sliceString(position - 1, position + 1);
  const previous = pair.charCodeAt(0), next = pair.charCodeAt(1);
  return previous >= 0xD800 && previous <= 0xDBFF && next >= 0xDC00 && next <= 0xDFFF;
}

/** Remove only incomplete code units; surrounding valid edits stay intact. */
export function incompleteScalars(text: Text): { from: number; to: number }[] {
  const value = text.toString();
  const changes: { from: number; to: number }[] = [];
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code >= 0xD800 && code <= 0xDBFF) {
      const next = value.charCodeAt(index + 1);
      if (next >= 0xDC00 && next <= 0xDFFF) index++;
      else changes.push({ from: index, to: index + 1 });
    } else if (code >= 0xDC00 && code <= 0xDFFF) changes.push({ from: index, to: index + 1 });
  }
  return changes;
}
