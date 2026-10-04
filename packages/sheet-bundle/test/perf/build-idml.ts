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

// Copied from plugin-draw's test/fixtures/build-idml.ts (the minimal IDML
// package builder its conformance fixtures stand on), trimmed to the
// envelope: the sheet perf workloads need one empty page to place into.

import { deflateRawSync } from "node:zlib";

interface IdmlEntry {
  name: string;
  data: string;
  /** mimetype must be STORED (uncompressed) + first, per the IDML
   *  package convention; everything else deflates. */
  store?: boolean;
}

function crc32(buf: Uint8Array): number {
  let c = ~0;
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i];
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}

/** Assemble a minimal ZIP (local headers + central directory + EOCD).
 *  Store or deflate per entry; CRC32 + sizes filled so a strict reader
 *  (the engine) accepts it. */
export function buildIdml(entries: IdmlEntry[]): Uint8Array {
  const enc = new TextEncoder();
  const chunks: Uint8Array[] = [];
  const central: {
    name: Uint8Array;
    crc: number;
    comp: number;
    raw: number;
    store: boolean;
    offset: number;
  }[] = [];
  let offset = 0;

  for (const e of entries) {
    const data = enc.encode(e.data);
    const crc = crc32(data);
    const store = !!e.store;
    const comp = store ? data : new Uint8Array(deflateRawSync(data));
    const nameBytes = enc.encode(e.name);
    const lfh = new Uint8Array(30 + nameBytes.length);
    const dv = new DataView(lfh.buffer);
    dv.setUint32(0, 0x04034b50, true);
    dv.setUint16(4, 20, true);
    dv.setUint16(8, store ? 0 : 8, true);
    dv.setUint32(14, crc, true);
    dv.setUint32(18, comp.length, true);
    dv.setUint32(22, data.length, true);
    dv.setUint16(26, nameBytes.length, true);
    lfh.set(nameBytes, 30);
    chunks.push(lfh, comp);
    central.push({
      name: nameBytes,
      crc,
      comp: comp.length,
      raw: data.length,
      store,
      offset,
    });
    offset += lfh.length + comp.length;
  }

  const cdStart = offset;
  for (const c of central) {
    const cd = new Uint8Array(46 + c.name.length);
    const dv = new DataView(cd.buffer);
    dv.setUint32(0, 0x02014b50, true);
    dv.setUint16(4, 20, true);
    dv.setUint16(6, 20, true);
    dv.setUint16(10, c.store ? 0 : 8, true);
    dv.setUint32(16, c.crc, true);
    dv.setUint32(20, c.comp, true);
    dv.setUint32(24, c.raw, true);
    dv.setUint16(28, c.name.length, true);
    dv.setUint32(42, c.offset, true);
    cd.set(c.name, 46);
    chunks.push(cd);
    offset += cd.length;
  }

  const eocd = new Uint8Array(22);
  const dv = new DataView(eocd.buffer);
  dv.setUint32(0, 0x06054b50, true);
  dv.setUint16(8, central.length, true);
  dv.setUint16(10, central.length, true);
  dv.setUint32(12, offset - cdStart, true);
  dv.setUint32(16, cdStart, true);
  chunks.push(eocd);

  let total = 0;
  for (const c of chunks) total += c.length;
  const out = new Uint8Array(total);
  let p = 0;
  for (const c of chunks) {
    out.set(c, p);
    p += c.length;
  }
  return out;
}

const MIME = "application/vnd.adobe.indesign-idml-package";

const empty = (tag: string): string =>
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n` +
  `<idPkg:${tag} xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0"/>`;

const CONTAINER =
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n` +
  `<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">` +
  `<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>`;

const GRAPHIC =
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n` +
  `<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">` +
  `<Color Self="Color/Black" Model="Process" Space="CMYK" ColorValue="0 0 0 100" Name="Black"/>` +
  `<Swatch Self="Swatch/None" Name="None"/></idPkg:Graphic>`;

const MASTER =
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n` +
  `<idPkg:MasterSpread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">` +
  `<MasterSpread Self="um" Name="A">` +
  `<Page Self="ump" Name="A" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>` +
  `</MasterSpread></idPkg:MasterSpread>`;

const BACKING =
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n` +
  `<idPkg:BackingStory xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">` +
  `<XmlStory Self="backing"/></idPkg:BackingStory>`;

const DESIGNMAP =
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n` +
  `<?aid style="50" type="document" readerVersion="6.0" featureSet="257" product="20.0(32)"?>\n` +
  `<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0" Self="d" StoryList="" Name="paged-draw-conformance.indd">\n` +
  `<idPkg:Graphic src="Resources/Graphic.xml"/>\n` +
  `<idPkg:Fonts src="Resources/Fonts.xml"/>\n` +
  `<idPkg:Styles src="Resources/Styles.xml"/>\n` +
  `<idPkg:Preferences src="Resources/Preferences.xml"/>\n` +
  `<idPkg:MasterSpread src="MasterSpreads/MasterSpread_um.xml"/>\n` +
  `<idPkg:Spread src="Spreads/Spread_us.xml"/>\n` +
  `<idPkg:BackingStory src="XML/BackingStory.xml"/>\n` +
  `</Document>`;

const STYLES_MINIMAL =
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n` +
  `<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">` +
  `<RootCharacterStyleGroup Self="rcs">` +
  `<CharacterStyle Self="CharacterStyle/$ID/[No character style]" Name="$ID/[No character style]"/>` +
  `</RootCharacterStyleGroup>` +
  `<RootParagraphStyleGroup Self="rps">` +
  `<ParagraphStyle Self="ParagraphStyle/$ID/[No paragraph style]" Name="$ID/[No paragraph style]"/>` +
  `</RootParagraphStyleGroup></idPkg:Styles>`;

/** Wrap a `<Spread>…</Spread>` body in the package envelope. The
 *  caller supplies the page-item XML (rectangles/polygons/lines);
 *  everything else is the shared minimal scaffold. */
export function packageWithSpread(
  spreadBody: string,
  opts: { styles?: string; fonts?: string } = {},
): Uint8Array {
  const spread =
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n` +
    `<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">\n` +
    `<Spread Self="us" PageCount="1" ItemTransform="1 0 0 1 0 0">\n` +
    `<Page Self="usp" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0" AppliedMaster="um"/>\n` +
    spreadBody +
    `\n</Spread>\n</idPkg:Spread>`;
  return buildIdml([
    { name: "mimetype", data: MIME, store: true },
    { name: "designmap.xml", data: DESIGNMAP },
    { name: "META-INF/container.xml", data: CONTAINER },
    { name: "Resources/Graphic.xml", data: GRAPHIC },
    { name: "Resources/Fonts.xml", data: opts.fonts ?? empty("Fonts") },
    { name: "Resources/Styles.xml", data: opts.styles ?? STYLES_MINIMAL },
    { name: "Resources/Preferences.xml", data: empty("Preferences") },
    { name: "MasterSpreads/MasterSpread_um.xml", data: MASTER },
    { name: "Spreads/Spread_us.xml", data: spread },
    { name: "XML/BackingStory.xml", data: BACKING },
  ]);
}
