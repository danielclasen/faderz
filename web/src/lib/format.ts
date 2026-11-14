// Readouts whose text changes keep one width: padded to a fixed number of
// characters, shown in the tabular monospace face with white-space: pre
// (class "fixed" in app.css). A right-aligned or centred text of another
// length would start elsewhere, which is a layout shift.

/** `text` padded at the start to `width` characters. */
export function fit(text: string, width: number): string {
  return text.padStart(width);
}
