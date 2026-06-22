/*
 * This file is part of paged (https://paged.media).
 *
 * paged is free software: you may redistribute it and/or modify it under the
 * terms of the GNU Affero General Public License, version 3, as published by
 * the Free Software Foundation, OR under the Paged Media Enterprise License
 * (PMEL), a commercial license available from And The Next GmbH. Full
 * copyright and license information is available in LICENSE.md, distributed
 * with this source code.
 *
 * paged is distributed in the hope that it will be useful, but WITHOUT ANY
 * WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
 * FOR A PARTICULAR PURPOSE. See the licenses for details.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    AGPL-3.0-only OR Paged Media Enterprise License (PMEL)
 */

// Formula-bar autocomplete — PURE prefix matching over the engine's
// registry-generated function name table (S-04 formula bar). The NAMES
// come from the engine (`engine.listFunctions()`, constitution §7); this
// module only matches a typed prefix against them. ZERO spreadsheet
// semantics — it never parses or evaluates a formula, it just answers
// "which function names start with the token the caret is on".
//
// The token the user is completing is the LAST run of function-name
// characters (`[A-Za-z0-9.]`) before the caret. Completing replaces that
// run with the chosen name (and an opening paren, the Excel affordance).
// Everything here is pure string surgery so the panel stays a thin view.

/** One function the formula bar can complete to — the engine's registry
 *  row, narrowed to what the completion UI shows (name + a thin arity
 *  hint). `maxArgs` null = variadic. */
export interface FunctionEntry {
  name: string;
  family: string;
  minArgs: number;
  maxArgs: number | null;
}

/** The completion token under the caret: the run of function-name chars
 *  (`[A-Za-z0-9.]`) ending AT the caret, with its start offset so a chosen
 *  completion can splice the name in. `text` is "" when the caret is not on
 *  such a run (e.g. right after a paren or operator). */
export interface CompletionToken {
  text: string;
  start: number;
  end: number;
}

/** A function name is `[A-Za-z0-9.]` (the registry name charset plus the
 *  digits/dot that appear mid-name, e.g. `T.DIST.2T`). The token scan uses
 *  the same class so a partially-typed name is captured whole. */
function isNameChar(ch: string): boolean {
  return /[A-Za-z0-9.]/.test(ch);
}

/** Extract the completion token ending at `caret` in `input`. Scans left
 *  over name chars from the caret. Returns `text === ""` when the char
 *  before the caret is not a name char (nothing to complete). Pure. */
export function completionTokenAt(input: string, caret: number): CompletionToken {
  const end = Math.max(0, Math.min(caret, input.length));
  let start = end;
  while (start > 0 && isNameChar(input[start - 1])) start -= 1;
  return { text: input.slice(start, end), end, start };
}

/** Prefix-match `entries` against `prefix` (case-insensitive — formula
 *  names are case-insensitive in the engine), returning at most `limit`
 *  matches. An empty prefix yields no suggestions (the bar only proposes
 *  once the user has typed a letter — never a full-registry dump). Results
 *  preserve the engine's order (already sorted by id) with EXACT matches
 *  surfaced first so a fully-typed name (e.g. "SUM") leads its own family
 *  (SUM before SUMIF). Pure — no host, no state. */
export function matchFunctions(
  entries: readonly FunctionEntry[],
  prefix: string,
  limit = 8,
): FunctionEntry[] {
  const p = prefix.toUpperCase();
  if (p.length === 0) return [];
  const exact: FunctionEntry[] = [];
  const rest: FunctionEntry[] = [];
  for (const e of entries) {
    const name = e.name.toUpperCase();
    if (name === p) exact.push(e);
    else if (name.startsWith(p)) rest.push(e);
    if (exact.length + rest.length >= entries.length) break;
  }
  return [...exact, ...rest].slice(0, limit);
}

/** Splice a chosen completion `name` into `input` at the token spanning
 *  `[token.start, token.end)`, appending an opening paren (the Excel
 *  affordance) and placing the caret just after it. Returns the new input
 *  + caret offset. Pure string surgery. */
export function applyCompletion(
  input: string,
  token: CompletionToken,
  name: string,
): { value: string; caret: number } {
  const head = input.slice(0, token.start);
  const tail = input.slice(token.end);
  const inserted = `${name}(`;
  return { value: head + inserted + tail, caret: head.length + inserted.length };
}

/** A short arity hint for the suggestion list (e.g. `"1+"` variadic,
 *  `"2"` fixed, `"2–4"` ranged). Pure formatting over the registry arity.
 *  A variadic (`maxArgs` null/undefined — Rust `Option::None` crosses
 *  serde-wasm-bindgen as `undefined`) shows `"N+"`. */
export function arityHint(entry: FunctionEntry): string {
  if (entry.maxArgs == null) return `${entry.minArgs}+`;
  if (entry.maxArgs === entry.minArgs) return `${entry.minArgs}`;
  return `${entry.minArgs}–${entry.maxArgs}`;
}
