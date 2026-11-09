// License text must not depend on checkout roots, path separators, locale or
// the line endings inside an upstream archive. Git stores this output as LF.
export function licenseNotices(values) {
  return [...new Set(Array.from(values, value => value.replace(/\r\n?/g, "\n")))]
    .sort((a, b) => a < b ? -1 : a > b ? 1 : 0)
    .join("\n--------------------\n\n");
}
