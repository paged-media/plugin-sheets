#!/usr/bin/env python3
# paged.sheet — Excel oracle lane, step 1: write one workbook per fn-corpus family.
#
# Usage:  python3 generate.py [family ...]      (default: every formula family)
# Output: $PAGED_ORACLE_BUILD (default oracle/excel/build/)<family>.xlsx
#
# One SHEET per golden case (`c1`, `c2`, ... in corpus order — see
# oracle_cases.load_family), so a case's setup can never leak into another
# case's ranges. Setup cells are written TYPED (number / bool / error / text /
# formula) by the same grammar the Rust runner applies, so Excel and the engine
# start from identical inputs. The probe formula sits at the case's host cell
# (default Z99, `@Addr` overrides).
#
# Every formula is written as a DYNAMIC-ARRAY formula (`cm="1"` + the XLDAPR
# metadata part). That is what modern Excel enters when a user types a formula,
# and the engine's semantics are the dynamic-array ones (it spills). Written as
# a legacy formula, Excel would apply implicit intersection to range arguments
# and the comparison would be against Excel 2016 behaviour instead.
#
# No cached values are written and calcPr asks for fullCalcOnLoad, so nothing
# Excel saves can be a value we put there.
#
# The xlsx is written by hand (stdlib zipfile) rather than with openpyxl
# because openpyxl cannot emit the dynamic-array metadata.

from __future__ import annotations

import sys
import zipfile
from xml.sax.saxutils import escape

import oracle_cases as oc

NS = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
RNS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"

CONTENT_TYPES_HEAD = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>
<Override PartName="/xl/metadata.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml"/>
"""

ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>
"""

STYLES = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="{NS}">
<fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>
<fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>
<borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>
<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
<cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs>
<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>
</styleSheet>
"""

METADATA = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<metadata xmlns="{NS}" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray">
<metadataTypes count="1"><metadataType name="XLDAPR" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1" cellMeta="1"/></metadataTypes>
<futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata>
<cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata>
</metadata>
"""


def col_letters(col: int) -> str:
    s = ""
    while col:
        col, r = divmod(col - 1, 26)
        s = chr(65 + r) + s
    return s


def cell_xml(addr: str, kind: str, payload) -> str:
    r = addr.upper()
    if kind == "blank":
        return ""
    if kind == "number":
        return f'<c r="{r}"><v>{payload}</v></c>'
    if kind == "bool":
        return f'<c r="{r}" t="b"><v>{1 if payload else 0}</v></c>'
    if kind == "error":
        return f'<c r="{r}" t="e"><v>{escape(payload)}</v></c>'
    if kind == "text":
        # Inline strings keep the generator free of a shared-string table;
        # xml:space keeps leading/trailing blanks (TRIM/LEN cases need them).
        return (f'<c r="{r}" t="inlineStr"><is><t xml:space="preserve">'
                f'{escape(payload)}</t></is></c>')
    if kind == "formula":
        f = escape(oc.to_file_formula(payload))
        return f'<c r="{r}" cm="1"><f t="array" ref="{r}">{f}</f></c>'
    raise ValueError(kind)


def sheet_xml(case: oc.Case, skip: bool) -> str:
    """One case's worksheet. `skip` = Excel cannot take this case (rejected
    formula or unrepresentable setup): the sheet is written EMPTY so the
    sheet numbering stays aligned with the case order."""
    cells: dict[tuple[int, int], str] = {}
    if skip:
        return (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
                f'<worksheet xmlns="{NS}"><sheetData/></worksheet>\n')
    for addr, raw in case.setup:
        if addr == "-" or addr.startswith("@"):
            continue
        kind, payload = oc.typed_setup(raw)
        x = cell_xml(addr, kind, payload)
        if x:
            cells[oc.a1(addr)] = x
    cells[oc.a1(case.host)] = cell_xml(case.host, "formula", case.formula)
    rows: dict[int, list[tuple[int, str]]] = {}
    for (r, c), x in cells.items():
        rows.setdefault(r, []).append((c, x))
    body = []
    for r in sorted(rows):
        body.append(f'<row r="{r}">' + "".join(x for _, x in sorted(rows[r])) + "</row>")
    return (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
            f'<worksheet xmlns="{NS}" xmlns:r="{RNS}"><sheetData>'
            + "".join(body) + "</sheetData></worksheet>\n")


def write_family(family: str) -> int:
    cases = oc.load_family(family)
    out = oc.build_dir() / f"{family}.xlsx"
    probe = oc.Case(family, "-", "locale-probe", oc.LOCALE_PROBE_FORMULA, [],
                    oc.LOCALE_PROBE_EXPECTED, host="A1", sheet=oc.LOCALE_PROBE_SHEET)
    sheets = cases + [probe]
    n = len(sheets)
    ct = CONTENT_TYPES_HEAD + "".join(
        f'<Override PartName="/xl/worksheets/sheet{i}.xml" '
        f'ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>\n'
        for i in range(1, n + 1)) + "</Types>\n"
    wb = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
          f'<workbook xmlns="{NS}" xmlns:r="{RNS}"><sheets>'
          + "".join(f'<sheet name="{c.sheet}" sheetId="{i}" r:id="rId{i}"/>'
                    for i, c in enumerate(sheets, 1))
          + '</sheets><calcPr calcId="0" fullCalcOnLoad="1"/></workbook>\n')
    rels = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            + "".join(f'<Relationship Id="rId{i}" Type="{RNS}/worksheet" '
                      f'Target="worksheets/sheet{i}.xml"/>' for i in range(1, n + 1))
            + f'<Relationship Id="rId{n + 1}" Type="{RNS}/styles" Target="styles.xml"/>'
            + f'<Relationship Id="rId{n + 2}" Type="{RNS}/sheetMetadata" Target="metadata.xml"/>'
            + "</Relationships>\n")
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", ct)
        z.writestr("_rels/.rels", ROOT_RELS)
        z.writestr("xl/workbook.xml", wb)
        z.writestr("xl/_rels/workbook.xml.rels", rels)
        z.writestr("xl/styles.xml", STYLES)
        z.writestr("xl/metadata.xml", METADATA)
        rejected = oc.rejected_keys()
        for i, c in enumerate(sheets, 1):
            skip = c.key in rejected or oc.unrepresentable(c) is not None
            z.writestr(f"xl/worksheets/sheet{i}.xml", sheet_xml(c, skip))
    print(f"{family}: {len(cases)} case(s) -> {out}")
    return len(cases)


def main(argv: list[str]) -> int:
    fams = argv or oc.families()
    total = sum(write_family(f) for f in fams)
    print(f"generated {total} case(s) in {len(fams)} workbook(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
