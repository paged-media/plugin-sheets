// The protocol-66 doors this bundle uses, FEATURE-DETECTED (Wave 9).
//
// The package pins stay at plugin-api/plugin-sdk 0.2.37 until protocol 66 is
// tagged and published, so the three additions are typed HERE, minimally,
// and every use has a fallback to the path a 0.64/0.65 host answers:
//
//   `MutationOutcome.minted` — every element a batch minted, in mint order,
//       with its `bindCreated` handle and its story (a text frame's
//       ParentStory). Detected by field PRESENCE on an applied outcome.
//       Fallback: the scene-tree / stories-collection diffs around the batch.
//   `deleteTable { storyId, tableId }` — a Mutation variant. Detected by
//       TRYING it: an older engine refuses the unknown variant, the refusal
//       is remembered per host and the old path (empty the table) runs.
//   `ElementGeometryItem.storyId` — a text frame's story on the geometry
//       read. Detected by field presence on a text frame's item. Fallback:
//       walk the stories and their frame chains.
//   `host.document.onWillSave` — the editor awaits its listeners before it
//       asks the engine for a .paged save's bytes. Probed with
//       `supports("document.onWillSave@1")`. Fallback: the debounced
//       persist and the flush on frame exit / deactivate.
//   In-batch TABLE handles — `$h:t` bound to an `insertTable` resolves in a
//       `tableId` / `table_id` position to the table's own id (core 66), so
//       a placement is one batch. No probe key: see `tableHandlesLikely`.
//
// Remove this module (and use the plugin-api types) when the pins move to
// the 66 canary.

import type {
  BundleHost,
  Disposable,
  ElementId,
  Mutation,
  MutationOutcome,
  SceneTreeNode,
} from "@paged-media/plugin-api";

/** One element a mutation minted (plugin-api 66 `MintedElement`). */
export interface MintedElement {
  handle: string | null;
  element: ElementId;
  storyId: string | null;
}

/** The `minted` list of an applied outcome, or null when the host did not
 *  send one (a pre-66 host-impl drops the engine's list). */
export function mintedOf(outcome: MutationOutcome): MintedElement[] | null {
  if (!outcome.applied) return null;
  const m = (outcome as { minted?: unknown }).minted;
  return Array.isArray(m) ? (m as MintedElement[]) : null;
}

/** The 66 `deleteTable` op, typed as a Mutation for a 0.2.37 contract. */
export function deleteTableOp(storyId: string, tableId: string): Mutation {
  return { op: "deleteTable", args: { storyId, tableId } } as unknown as Mutation;
}

/** What this session has learned about the host's 66 doors. `undefined` =
 *  not yet known. Keyed by the host object a session holds. */
interface Doors66 {
  deleteTable?: boolean;
  minted?: boolean;
  geometryStoryId?: boolean;
  /** Whether a one-batch placement with table handles applied (true) or
   *  was refused (false) on this host. */
  tableHandles?: boolean;
}
const known = new WeakMap<object, Doors66>();

export function doors66(host: BundleHost): Doors66 {
  let d = known.get(host);
  if (!d) {
    d = {};
    known.set(host, d);
  }
  return d;
}

/** Record whether an applied outcome carried `minted` (the host-impl either
 *  copies it through or it never does). */
export function noteMinted(host: BundleHost, outcome: MutationOutcome): MintedElement[] | null {
  const m = mintedOf(outcome);
  if (outcome.applied) doors66(host).minted = m !== null;
  return m;
}

/** Whether a refusal is the engine not knowing the `op` variant at all (a
 *  pre-66 wire), as opposed to refusing this particular call. */
export function isUnknownVariant(outcome: MutationOutcome, op: string): boolean {
  if (outcome.applied) return false;
  let text: string;
  try {
    text = JSON.stringify(outcome.error);
  } catch {
    return false;
  }
  return text.includes("unknown variant") && text.includes(op);
}

/** What a will-save listener receives (plugin-api 66 `WillSaveEvent`). */
export interface WillSaveEvent {
  format: "paged";
}

/** Register `listener` on the host's will-save door (66), or return null
 *  when the host has none. The editor awaits every listener (bounded) before
 *  it takes a .paged save's bytes, so a listener that settles once its
 *  state is in the document makes that state part of the file. */
export function onWillSave(
  host: BundleHost,
  listener: (e: WillSaveEvent) => void | Promise<void>,
): Disposable | null {
  if (typeof host.supports !== "function" || !host.supports("document.onWillSave@1")) return null;
  const door = (host.document as unknown as {
    onWillSave?: (l: (e: WillSaveEvent) => void | Promise<void>) => Disposable;
  }).onWillSave;
  if (typeof door !== "function") return null;
  return door.call(host.document, listener);
}

/** Whether a one-batch placement that addresses the table it inserts by
 *  handle (`$h:t` in `tableId` / `table_id` positions — core 66) is worth
 *  sending. There is no supports() key for an engine behaviour, and a
 *  refused attempt is not free on a 0.64 engine (rolling back the batch's
 *  `insertTextFrame` leaves its story behind in the model), so it is sent
 *  only on POSITIVE evidence of a 66 host — a 66 door it answered (the will-
 *  save door, `minted` on an outcome, `storyId` on a frame's geometry) —
 *  and never again on a host that refused one. The placement reads its
 *  ids from `minted`, so a host known not to send it is excluded too. */
export function tableHandlesLikely(host: BundleHost): boolean {
  const d = doors66(host);
  if (d.tableHandles !== undefined) return d.tableHandles;
  if (d.minted === false) return false;
  return (
    d.minted === true ||
    d.geometryStoryId === true ||
    (typeof host.supports === "function" && host.supports("document.onWillSave@1"))
  );
}

/** The story of a text-frame geometry item (66), or undefined. */
export function geometryStoryId(item: unknown): string | undefined {
  const s = (item as { storyId?: unknown } | null)?.storyId;
  return typeof s === "string" && s.length > 0 ? s : undefined;
}

// ── The fallback half of `minted`: what a batch made, by scene-tree diff.

/** Every addressable element id in a scene tree, keyed by kind:id. */
function treeIds(nodes: readonly SceneTreeNode[], out = new Map<string, ElementId>()) {
  for (const n of nodes) {
    if (n.id && typeof n.id.id === "string") out.set(`${n.id.kind}:${n.id.id}`, n.id);
    if (n.children) treeIds(n.children, out);
  }
  return out;
}

/** The scene tree's element ids, or null when the host cannot answer. */
export async function readTreeIds(host: BundleHost): Promise<Map<string, ElementId> | null> {
  if (typeof host.document.tree !== "function") return null;
  try {
    return treeIds(await host.document.tree());
  } catch {
    return null;
  }
}

/** A batch that will mint elements whose ids the caller needs: read the tree
 *  BEFORE it unless the host is known to send `minted` (`null` = no read). */
export async function beforeMint(host: BundleHost): Promise<Map<string, ElementId> | null> {
  return doors66(host).minted === true ? null : readTreeIds(host);
}

/** The elements `outcome`'s batch minted, filtered by `keep`: the 66
 *  `minted` list, else the scene-tree diff against `before` (one more read).
 *  Empty when neither answers. */
export async function mintedElements(
  host: BundleHost,
  outcome: MutationOutcome,
  before: Map<string, ElementId> | null,
  keep: (id: ElementId) => boolean,
): Promise<ElementId[]> {
  const minted = noteMinted(host, outcome);
  if (minted) return minted.map((m) => m.element).filter(keep);
  if (!before || !outcome.applied) return [];
  const after = await readTreeIds(host);
  if (!after) return [];
  return [...after].filter(([k]) => !before.has(k)).map(([, id]) => id).filter(keep);
}

/** A wheel delivered to the ACTIVE edit context (editor `onContentWheel`,
 *  ahead of plugin-api): `delta` is the scroll in frame-content points (the
 *  screen delta divided by the camera scale), x right / y down. Return true
 *  when the context scrolled (the canvas then does not pan). Cmd/Ctrl wheel
 *  is the host's zoom and never arrives. */
export interface ContentWheelEvent {
  contentPoint: [number, number];
  elementId: string;
  delta: [number, number];
  modifiers: { shift: boolean; alt: boolean; cmd: boolean; ctrl: boolean };
}

/** The edit-context hook the editor honours today (it spreads the
 *  contribution into its registry); plugin-api names it from 0.2.38. */
export interface ContentWheelHook {
  onContentWheel?(e: ContentWheelEvent): boolean;
}
