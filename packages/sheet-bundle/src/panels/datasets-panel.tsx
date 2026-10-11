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

// The datasets panel (S-15) — the CONSUMER side of the data-provider
// contract. Lists the governed datasets the platform offers in the
// "dataset" category (host.dataProviders.discover) and lets the author
// SOURCE a sheet from one (session.sourceFromDataset → a fresh workbook
// seeded from the provider's resolved snapshot). The sheet never fetches:
// it receives an already-resolved RecordSet the platform hands it (§1.1 —
// paged.data owns the network + the §11 consent; the consumer reads
// cached data). ISOLATION (§2.1): this consumes ONLY host.dataProviders;
// it never learns which plugin backs a dataset, never imports paged.data.
//
// A React expert-leaf factory closing over BundleHost + the session,
// built from host surfaces + React ONLY (no @paged-media/shell). Honest
// empty state when no datasets are discovered (paged.data absent / no
// registry wired). Rows read by a display name derived from the provider id
// (the contract carries no label), never the raw id. Token-layer styling (--pg-*, --space-*, --font-mono,
// --radius-*) mirrors the workbook panel so it reads native in both themes.

import {
  useCallback,
  useEffect,
  useState,
  type CSSProperties,
  type ReactElement,
} from "react";

import type { BundleHost } from "@paged-media/plugin-api";

import type { WorkbookSession } from "../session";
import { useConfirm } from "./confirm";

// ---------------------------------------------------------------- styles

const kicker: CSSProperties = {
  font: "700 10px var(--font-sans, sans-serif)",
  letterSpacing: "var(--tracking-wide, 0.14em)",
  textTransform: "uppercase",
  color: "var(--pg-muted-fg)",
  margin: "var(--space-3, 12px) 0 var(--space-1, 4px)",
};

const note: CSSProperties = {
  margin: "var(--space-3, 12px) 0 0",
  font: "10px/1.5 var(--font-sans, sans-serif)",
  color: "var(--pg-muted-fg)",
};

const row: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: "var(--space-2, 8px)",
  padding: "6px 8px",
  border: "1px solid var(--pg-border)",
  borderRadius: "var(--radius-sm, 4px)",
  background: "var(--pg-bg)",
};

const nameStyle: CSSProperties = {
  font: "500 12px var(--font-sans, sans-serif)",
  color: "var(--pg-fg)",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
};

const metaStyle: CSSProperties = {
  font: "10px var(--font-sans, sans-serif)",
  color: "var(--pg-muted-fg)",
};

const sourceButton: CSSProperties = {
  font: "500 12px var(--font-sans, sans-serif)",
  color: "var(--primary-fg, #fff)",
  background: "var(--pg-primary)",
  border: "none",
  borderRadius: "var(--radius-md, 6px)",
  padding: "4px 10px",
  cursor: "pointer",
  flex: "0 0 auto",
};

/** A dataset's display name from its provider id: the id is the only name
 *  the data-provider contract carries, so "pricing-dataset" reads
 *  "Pricing" and "q_all" reads "Q all". The id stays in the tooltip. */
export function datasetLabel(id: string): string {
  const words = id
    .replace(/^.*[/:]/, "") // a namespaced id: keep its last segment
    .replace(/[-_.]dataset$/i, "")
    .replace(/[-_.]+/g, " ")
    .trim();
  if (!words) return id;
  return words[0].toUpperCase() + words.slice(1);
}

/** "3 fields: name, price, qty" (the first few field names). */
function fieldSummary(fields: readonly { name: string }[]): string {
  const n = fields.length;
  const head = `${n} field${n === 1 ? "" : "s"}`;
  if (n === 0) return head;
  const names = fields.slice(0, 3).map((f) => f.name).join(", ");
  return `${head}: ${names}${n > 3 ? ", …" : ""}`;
}

// ----------------------------------------------------------------- panel

export function makeDatasetsPanel(
  host: BundleHost,
  session: WorkbookSession,
): () => ReactElement {
  return function DatasetsPanel(): ReactElement {
    // Re-render on session changes (a sourced sheet, a stale signal) AND
    // when the panel mounts (discover is a synchronous registry read).
    const [, force] = useState(0);
    useEffect(() => {
      const sub = session.onDidChange(() => force((n) => n + 1));
      return () => sub.dispose();
    }, []);

    // The governed datasets in the "dataset" category — schema + revision,
    // NO rows. [] when no registry is wired (paged.data absent / headless).
    const datasets = session.discoverDatasets();
    const st = session.state();

    const onSource = useCallback((id: string) => {
      host.log.info(`sourcing sheet from dataset "${id}"`);
      void session.sourceFromDataset(id);
    }, []);
    // Using a dataset starts a fresh workbook: with another workbook open
    // that replaces it, so the button asks for a second click first.
    const { armed, confirm } = useConfirm();
    const workbookOpen = session.sheets().length > 0;

    return (
      <div
        data-sheet-panel="datasets"
        style={{
          padding: "var(--space-3, 12px)",
          display: "flex",
          flexDirection: "column",
        }}
      >
        <div style={{ ...kicker, marginTop: 0 }}>Datasets</div>

        {datasets.length === 0 ? (
          // Honest empty state — §2.1 graceful absence: paged.data not
          // installed / no shared registry wired ⇒ no governed sources.
          <p data-datasets-empty style={{ ...note, marginTop: 0 }}>
            No datasets yet. Publish a query as a dataset in paged.data, and
            it appears here.
          </p>
        ) : (
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              gap: "var(--space-2, 8px)",
            }}
          >
            {datasets.map((d) => {
              const sourced = st.dataSource?.providerId === d.id;
              const stale = sourced && st.dataSource?.stale === true;
              return (
                <div data-dataset-row data-dataset-id={d.id} key={d.id} style={row}>
                  <div style={{ minWidth: 0 }}>
                    <div data-dataset-name style={nameStyle} title={d.id}>
                      {datasetLabel(d.id)}
                    </div>
                    <div data-dataset-fields={d.schema.fields.length} style={metaStyle}>
                      {fieldSummary(d.schema.fields)}
                      {sourced && (stale ? " · update available" : " · in use")}
                    </div>
                  </div>
                  <button
                    type="button"
                    data-dataset-source
                    data-armed={armed === d.id ? "" : undefined}
                    onClick={() => {
                      if (workbookOpen && !sourced) confirm(d.id, () => onSource(d.id));
                      else onSource(d.id);
                    }}
                    title={
                      stale
                        ? "Replace the sheet with the newer revision"
                        : workbookOpen && !sourced
                          ? "Replaces the open workbook with this dataset; this can't be undone"
                          : "Start a new workbook from this dataset"
                    }
                    style={sourceButton}
                  >
                    {armed === d.id ? "Confirm replace" : stale ? "Update" : "Use"}
                  </button>
                </div>
              );
            })}
          </div>
        )}

        {/* S-15 honesty: a dataset-sourced sheet is a COMMITTED snapshot —
            the cell values travel with the document; the sheet never
            fetches (paged.data owns the network). A refresh is an explicit
            re-source, never automatic on open (§1.1 / the consumer RFC). */}
        <p data-datasets-honesty style={note}>
          A sheet made from a dataset is a snapshot saved with the document;
          choose Update when a newer revision is available.
        </p>
      </div>
    );
  };
}
