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
// registry wired). Token-layer styling (--pg-*, --space-*, --font-mono,
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

const idStyle: CSSProperties = {
  font: "12px var(--font-mono, monospace)",
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
            No datasets — install or enable paged.data, or no data-provider
            registry is wired in this host.
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
                    <div style={idStyle} title={d.id}>
                      {d.id}
                    </div>
                    <div data-dataset-fields={d.schema.fields.length} style={metaStyle}>
                      {d.schema.fields.length} field
                      {d.schema.fields.length === 1 ? "" : "s"}
                      {sourced && (stale ? " · update available" : " · sourced")}
                    </div>
                  </div>
                  <button
                    type="button"
                    data-dataset-source
                    onClick={() => onSource(d.id)}
                    style={sourceButton}
                  >
                    {stale ? "Re-source" : "Source"}
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
          A sourced sheet is a committed snapshot of the dataset — it travels
          with the document and is never re-fetched automatically. Re-source
          to pull a newer revision.
        </p>
      </div>
    );
  };
}
