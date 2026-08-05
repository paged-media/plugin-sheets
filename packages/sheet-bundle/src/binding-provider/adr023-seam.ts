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

// ---------------------------------------------------------- the door
// ESCAPE HATCH, named rather than hidden — and the SAME shape
// plugin-draw's `binding-provider/adr023-seam.ts` uses, deliberately:
// one file owns the skew, and a canary repin is a deletion.
//
// THE SKEW: this repo installs the PUBLISHED
// `@paged-media/plugin-{api,sdk}@0.2.25-canary.0`, which predates ADR
// 023 phase A (plugin-sdk `ee778c5`). So
// `host.contribute.bindingProvider` is not on the installed
// `ContributionSurface`, and `BindingProvider` / `BindingRead` /
// `BindingWrite` / `BindingCollection` are not exported. The mirrors
// below are the COMMITTED contract (`plugin-api/src/binding-provider.ts`)
// narrowed to the lanes this provider actually uses — the v58-wire
// precedent: a cast that points at a contract which EXISTS and is
// COMMITTED, not at a hope.
//
// When the canary bumps, this file becomes: delete the mirrors, import
// the real types, drop one cast. Nothing else in this repo touches the
// binding-provider seam.
//
// THE DEGRADATION IS LOAD-BEARING: `registerBindingProvider` probes for
// the door AND for a wired registry, and returns `null` when either is
// missing. On an older editor paged.sheet simply contributes no
// provider — the host Swatches panel keeps reading core, which is the
// pre-ADR behaviour. Never a throw, never a silently dead registration.

import type {
  BundleHost,
  CollectionName,
  Disposable,
  ElementId,
  Mutation,
  MutationOutcome,
  PropertyPath,
  Value,
} from "@paged-media/plugin-api";

// ------------------------------------------------------ local mirrors

/** Re-exported so a provider imports its whole vocabulary from the ONE
 *  file that owns the skew (the repin then touches nothing else). */
export type { PropertyPath, Value };

/** Mirror of the contract's `BindingTarget`. */
export type BindingTarget =
  | { kind: "selection"; scope: "element" | "content" }
  | { kind: "element"; id: ElementId }
  | { kind: "row"; collection: CollectionName; id: string };

/** Mirror of `BindingRead` (= `BindingResolved | BindingDecline`).
 *
 *  Unused by the swatches provider — it serves the DOCUMENT-SCOPED lanes
 *  only, because core models no swatch `PropertyPath` at all. The TEXT
 *  provider (ADR 023's third proof consumer, the VALUE axis) is what
 *  finally exercises it, and all four states with it: `value` for a
 *  uniform cell selection, `mixed` where the cells disagree, `absent`
 *  for the paths a cell does not model, `decline` when there is no
 *  workbook to read at all. */
export type BindingRead =
  | { kind: "value"; value: Value }
  | { kind: "mixed" }
  | { kind: "absent"; reason?: string }
  | { kind: "decline"; reason?: string };

/** Mirror of `BindingWrite`. A REFUSED write is `{applied:false, error}`
 *  INSIDE `applied`, not a decline — the provider owned it and said no,
 *  which is a different fact from "not mine". */
export type BindingWrite =
  | { kind: "applied"; outcome: MutationOutcome }
  | { kind: "decline"; reason?: string };

/** Mirror of `BindingCollection`. */
export type BindingCollection =
  | { kind: "rows"; rows: readonly unknown[] }
  | { kind: "decline"; reason?: string };

/** Mirror of `BindingProviderScope`. Every member is a CLOSED core
 *  union — the vocabulary rule: a provider addresses core's vocabulary
 *  and nothing else. */
export interface BindingProviderScope {
  paths?: readonly PropertyPath[];
  /** The subset of `paths` that accepts WRITES; omitted ⇒ all of them.
   *  Added to the contract BY this repo's text provider (plugin-sdk
   *  `binding-provider.ts` §"writablePaths", DESIGN.md §18.11): the
   *  sheet engine has no cell-style write API, so paged.sheet reads the
   *  whole Character/Paragraph surface and writes none of it, and the
   *  host needs that as a DECLARATION — `writeProperty`'s absence is a
   *  missing callback, which never reaches `activeProviders()`. */
  writablePaths?: readonly PropertyPath[];
  collections?: readonly CollectionName[];
  ops?: readonly string[];
}

/** Mirror of `BindingProvider`, narrowed to the lanes this repo's two
 *  providers need. `writeProperty` is deliberately absent from BOTH:
 *  the swatches provider writes structurally through `applyMutation`,
 *  and the text provider writes nothing at all and declares so. */
export interface BindingProvider {
  provides: BindingProviderScope;
  readProperty?(request: {
    path: PropertyPath;
    target: BindingTarget;
  }): BindingRead | Promise<BindingRead>;
  readCollection?(request: {
    collection: CollectionName;
  }): BindingCollection | Promise<BindingCollection>;
  applyMutation?(mutation: Mutation): BindingWrite | Promise<BindingWrite>;
}

/** Mirror of `BindingProviderHandle`. */
export interface BindingProviderHandle extends Disposable {
  invalidate(): void;
}

// ------------------------------------------------------------- probes

/** The host-side shape the cast targets. */
type ContributeWithBindingProvider = {
  bindingProvider?: (
    contextType: string,
    provider: BindingProvider,
  ) => BindingProviderHandle;
};

/**
 * Does this host know the ADR 023 phase-A door AND has it wired a
 * registry? Two different facts, and both matter:
 *
 *   · `contribute.bindingProvider` missing ⇒ the SDK predates phase A;
 *   · present but `bindings.provider@1` false ⇒ the SDK has the door but
 *     the HOST APP injected no registry, so the door warns and returns
 *     an inert handle and nothing would ever consult the provider.
 *
 * Registering into the second case is harmless but pointless, so the
 * probe requires both and the caller logs which one failed.
 */
export function supportsBindingProviders(host: BundleHost): boolean {
  const contribute = host.contribute as unknown as ContributeWithBindingProvider;
  if (typeof contribute.bindingProvider !== "function") return false;
  try {
    return host.supports("bindings.provider@1");
  } catch {
    return false;
  }
}

/**
 * The ONE `contribute.bindingProvider` call in this bundle. `null` when
 * the host cannot consult a provider at all.
 */
export function registerBindingProvider(
  host: BundleHost,
  contextType: string,
  provider: BindingProvider,
): BindingProviderHandle | null {
  if (!supportsBindingProviders(host)) {
    host.log.info(
      `binding provider for "${contextType}" not registered — this host ` +
        `has no ADR-023 binding-provider registry (the shared Swatches ` +
        `panel will read core, which is the pre-ADR behaviour)`,
    );
    return null;
  }
  const contribute = host.contribute as unknown as ContributeWithBindingProvider;
  return contribute.bindingProvider!(contextType, provider);
}
