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
//
// Remove this module (and use the plugin-api types) when the pins move to
// the 66 canary.

import type {
  BundleHost,
  ElementId,
  Mutation,
  MutationOutcome,
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

/** The story of a text-frame geometry item (66), or undefined. */
export function geometryStoryId(item: unknown): string | undefined {
  const s = (item as { storyId?: unknown } | null)?.storyId;
  return typeof s === "string" && s.length > 0 ? s : undefined;
}
