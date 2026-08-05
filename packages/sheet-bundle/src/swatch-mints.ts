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

// THE READ every lane that mints a document swatch owes the document.
//
// AN IDEMPOTENT-*LOOKING* MINT INSIDE A BATCH IS NOT IDEMPOTENT. paged.sheet
// mints swatches at deterministic, content-addressed ids (`palette.ts`), so a
// second lowering of the same chart / range / colour asks for ids the document
// already has. Core REFUSES a `createSwatch` whose `selfId` exists, and a
// refused child fails the WHOLE `Operation::Batch` — every sibling is rolled
// back. Measured against core's real engine over the exact ops this bundle
// emits: lowering a chart twice, or lowering ANY second chart (they share the
// axis grey), left the scene tree unchanged at 18 nodes — the second chart
// never landed at all.
//
// So: READ FIRST, MINT ONLY WHAT IS ABSENT. And when the read itself fails,
// mint NOTHING — `null` is not "the document has none". The same batch with
// its mints stripped still applies (a colorRef naming a missing swatch is a
// paint miss, not an op error), so the geometry lands and only the colour
// degrades. NEVER GAMBLE THE BATCH TO SAVE A COLOUR.
//
// The pure half of this rule — which mints to emit given what we know — is
// `swatchMintOps` in sheet-host-model's `palette.ts`. This module is the
// impure half: the one host round-trip, shared so the page lower and the chart
// lower cannot answer the question differently.

import type { BundleHost } from "@paged-media/plugin-api";
import type { KnownSwatchIds } from "../../sheet-host-model/src";

/**
 * The document's current swatch ids, or `null` when the READ ITSELF failed
 * (which is NOT the same as "there are none" — the caller passes this
 * straight to a `swatchMintOps` consumer, which mints nothing for `null`).
 */
export async function readKnownSwatchIds(
  host: BundleHost,
): Promise<KnownSwatchIds> {
  try {
    const rows = await host.document.collection<{ selfId: string }>("swatches");
    return new Set(rows.map((s) => s.selfId));
  } catch (err) {
    host.log.warn("swatch mints: document swatch read failed", err);
    return null;
  }
}
