import { EditorState, Extension, Facet, StateEffect, StateField, Text, Transaction } from "@codemirror/state";
import { incompleteScalars, scalarText, splitsScalar } from "./unicode";

export const MAX_DOCUMENT_BYTES = 10 * 1024 * 1024;
export const MAX_TRANSACTION_PATCHES = 4096;
const encoder = new TextEncoder();
interface Budget {
  bytes: number;
  rawBytes: number;
  newlineWidths: number[];
  preferredWidth: number;
  patches: number;
}
const initialBudget = Facet.define<Budget, Budget>({ combine: values => values[0] });
const estimates = new WeakMap<Transaction, Budget>();
const rebasedBudget = StateEffect.define<Budget>();

export function stringBytes(value: string): number { return encoder.encode(value).length; }

function textBytes(text: Text, from = 0, to = text.length): number {
  let bytes = 0;
  for (const part of text.iterRange(from, to)) bytes += stringBytes(part);
  return bytes;
}

function estimate(transaction: Transaction): Budget {
  const existing = estimates.get(transaction);
  if (existing) return existing;
  const previous = transaction.startState.field(documentBudget);
  let bytes = previous.bytes;
  let rawBytes = previous.rawBytes;
  let patches = 0;
  let recountUnicode = false;
  const newlineChanges: { from: number; to: number; inserted: number }[] = [];
  transaction.changes.iterChanges((from, to, _newFrom, _newTo, inserted) => {
    patches++;
    if (patches <= MAX_TRANSACTION_PATCHES) {
      recountUnicode ||= !scalarText(inserted.toString())
        || splitsScalar(transaction.startState.doc, from) || splitsScalar(transaction.startState.doc, to);
      const delta = textBytes(inserted) - textBytes(transaction.startState.doc, from, to);
      const firstLine = transaction.startState.doc.lineAt(from).number - 1;
      const lastLine = transaction.startState.doc.lineAt(to).number - 1;
      let removedOverhead = 0;
      for (let index = firstLine; index < lastLine; index++) removedOverhead += previous.newlineWidths[index] - 1;
      bytes += delta;
      rawBytes += delta - removedOverhead + (inserted.lines - 1) * (previous.preferredWidth - 1);
      if (firstLine !== lastLine || inserted.lines > 1) newlineChanges.push({ from: firstLine, to: lastLine, inserted: inserted.lines - 1 });
    }
  });
  if (recountUnicode) {
    // UTF-8 byte counts are not additive across a temporarily split surrogate.
    // Ordinary complete Unicode transactions keep the incremental fast path.
    const correction = textBytes(transaction.newDoc) - bytes;
    bytes += correction;
    rawBytes += correction;
  }
  let newlineWidths = previous.newlineWidths;
  if (rawBytes <= MAX_DOCUMENT_BYTES && patches <= MAX_TRANSACTION_PATCHES && newlineChanges.length) {
    // Ordinary character typing reuses this list. Newline transactions make one
    // copy then splice only changed spans backwards, preserving untouched EOLs.
    newlineWidths = newlineWidths.slice();
    for (const change of newlineChanges.reverse()) {
      // Avoid spreading a pasted millions-of-lines array into function arguments.
      newlineWidths.splice(change.from, change.to - change.from);
      if (change.inserted) {
        const insertedWidths = new Array<number>(change.inserted).fill(previous.preferredWidth);
        newlineWidths = newlineWidths.slice(0, change.from).concat(insertedWidths, newlineWidths.slice(change.from));
      }
    }
  }
  const result = { bytes, rawBytes, patches, newlineWidths, preferredWidth: previous.preferredWidth };
  estimates.set(transaction, result);
  return result;
}

export const documentBudget = StateField.define<Budget>({
  create: state => state.facet(initialBudget),
  update: (budget, transaction) => transaction.effects.find(effect => effect.is(rebasedBudget))?.value
    ?? (transaction.docChanged ? estimate(transaction) : budget)
});

function budgetFor(rawText: string): Budget {
  const newlineWidths: number[] = [];
  let crlf = 0, lf = 0, cr = 0;
  for (const match of rawText.matchAll(/\r\n|\r|\n/g)) {
    newlineWidths.push(match[0].length);
    if (match[0] === "\r\n") crlf++;
    else if (match[0] === "\r") cr++;
    else lf++;
  }
  const rawBytes = stringBytes(rawText);
  const preferredWidth = crlf > 0 && crlf >= lf && crlf >= cr ? 2 : 1;
  return { bytes: rawBytes - crlf, rawBytes, newlineWidths, preferredWidth, patches: 0 };
}

export function resetDocumentBudget(rawText: string): StateEffect<Budget> {
  return rebasedBudget.of(budgetFor(rawText));
}

export function documentLimits(rawText: string, rejected: (message: string) => void, accepted: () => void): Extension {
  return [
    initialBudget.of(budgetFor(rawText)),
    documentBudget,
    EditorState.transactionFilter.of(transaction => {
      if (!transaction.docChanged || transaction.startState.readOnly) return transaction;
      const { rawBytes, patches } = estimate(transaction);
      if (rawBytes > MAX_DOCUMENT_BYTES) {
        rejected("编辑未应用：笔记上限为 10 MiB，请减少粘贴内容。");
        const rollback = incompleteScalars(transaction.startState.doc);
        if (rollback.length) {
          return { changes: rollback, annotations: Transaction.addToHistory.of(false) };
        }
        return [];
      }
      if (patches > MAX_TRANSACTION_PATCHES) {
        rejected("编辑未应用：单次操作最多 4096 处修改，请分段格式化。");
        return [];
      }
      accepted();
      return transaction;
    })
  ];
}
