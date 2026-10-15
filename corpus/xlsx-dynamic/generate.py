#!/usr/bin/env python3
# Dynamic-array fixture: what Excel itself writes for a spilled formula.
#
#   python3 corpus/xlsx-dynamic/generate.py
#       writes dynamic-arrays.xlsx — the INPUT: every formula a dynamic-array
#       formula (cm="1" + the XLDAPR metadata part), no cached values,
#       fullCalcOnLoad. stdlib only, byte-stable.
#
#   PAGED_ORACLE_BUILD=<dir> sheet-conformance/oracle/excel/drive.sh dynamic-arrays
#       (maintainer, macOS + Excel 16, with the input copied into <dir>)
#       lets Excel compute and save it as <dir>/dynamic-arrays.excel.xlsx.
#
#   python3 corpus/xlsx-dynamic/generate.py --scrub <dir>/dynamic-arrays.excel.xlsx
#       copies Excel's save here as dynamic-arrays.excel.xlsx with the two
#       machine-specific values blanked (docProps/core.xml lastModifiedBy and
#       the workbook's x15ac:absPath). Every other part stays byte-for-byte
#       what Excel wrote.
#
# The cells: the three SEQUENCE anchors of a 24-row day-book, a 4-row
# SEQUENCE, SORT / UNIQUE / FILTER / TRANSPOSE over a small source block,
# two dynamic arrays whose root is not a function call (H1:H6*2, H1:H3), a
# blocked spill (S2 holds a value), a one-cell array, the spill-range
# operator (SUM(A8#)) and a 2-D SEQUENCE.

import os
import re
import sys
import zipfile
from xml.sax.saxutils import escape

HERE = os.path.dirname(os.path.abspath(__file__))
FIXED_DATE = (1980, 1, 1, 0, 0, 0)
NS = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
RNS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"

CT = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
<Override PartName="/xl/metadata.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml"/>
</Types>
"""
ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>
"""
WB_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata" Target="metadata.xml"/>
</Relationships>
"""
WB = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="{NS}" xmlns:r="{RNS}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets><calcPr calcId="191029" fullCalcOnLoad="1"/></workbook>
"""
METADATA = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<metadata xmlns="{NS}" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray">
<metadataTypes count="1"><metadataType name="XLDAPR" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1" cellMeta="1"/></metadataTypes>
<futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata>
<cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata>
</metadata>
"""


def col_num(letters):
    n = 0
    for ch in letters:
        n = n * 26 + (ord(ch) - 64)
    return n


def sheet_xml():
    cells = {}

    def put(addr, xml):
        i = 0
        while addr[i].isalpha():
            i += 1
        cells[(int(addr[i:]), col_num(addr[:i]))] = xml

    def num(addr, v):
        put(addr, f'<c r="{addr}"><v>{v}</v></c>')

    def text(addr, s):
        put(addr, f'<c r="{addr}" t="inlineStr"><is><t>{escape(s)}</t></is></c>')

    def dyn(addr, f):
        put(addr, f'<c r="{addr}" cm="1"><f t="array" ref="{addr}">{escape(f)}</f></c>')

    for i, (g, h) in enumerate(zip("bacabd", [5, 2, 8, 1, 9, 4]), start=1):
        text(f"G{i}", g)
        num(f"H{i}", h)
    dyn("A8", "_xlfn.SEQUENCE(24)")
    dyn("B8", "_xlfn.SEQUENCE(24,1,1840,7)")
    dyn("C8", "_xlfn.SEQUENCE(24,1,620,13)")
    dyn("E1", "_xlfn.SEQUENCE(4,1,10,10)")
    dyn("J1", "_xlfn._xlws.SORT(H1:H6)")
    dyn("K1", "_xlfn.UNIQUE(G1:G6)")
    dyn("L1", "_xlfn._xlws.FILTER(H1:H6,H1:H6>3)")
    dyn("M1", "TRANSPOSE(H1:H3)")
    dyn("Q1", "H1:H6*2")
    dyn("R1", "H1:H3")
    dyn("S1", "_xlfn.SEQUENCE(3)")
    text("S2", "x")
    dyn("W1", "_xlfn.SEQUENCE(1)")
    dyn("U1", "SUM(_xlfn.ANCHORARRAY(A8))")
    dyn("Y1", "_xlfn.SEQUENCE(2,3)")
    put("U3", '<c r="U3"><f>SUM(H1:H6)</f></c>')

    rows = {}
    for (r, _), xml in sorted(cells.items()):
        rows.setdefault(r, []).append(xml)
    data = "".join(f'<row r="{r}">{"".join(x)}</row>' for r, x in sorted(rows.items()))
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        f'<worksheet xmlns="{NS}" xmlns:r="{RNS}"><sheetData>{data}</sheetData></worksheet>\n'
    )


def write_zip(path, members):
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, body in members:
            info = zipfile.ZipInfo(name, date_time=FIXED_DATE)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, body)


def generate():
    out = os.path.join(HERE, "dynamic-arrays.xlsx")
    write_zip(
        out,
        [
            ("[Content_Types].xml", CT),
            ("_rels/.rels", ROOT_RELS),
            ("xl/workbook.xml", WB),
            ("xl/_rels/workbook.xml.rels", WB_RELS),
            ("xl/worksheets/sheet1.xml", sheet_xml()),
            ("xl/metadata.xml", METADATA),
        ],
    )
    print("wrote", out)


def scrub(src):
    out = os.path.join(HERE, "dynamic-arrays.excel.xlsx")
    members = []
    with zipfile.ZipFile(src) as z:
        for info in z.infolist():
            body = z.read(info.filename)
            if info.filename == "docProps/core.xml":
                body = re.sub(
                    rb"<cp:lastModifiedBy>[^<]*</cp:lastModifiedBy>",
                    b"<cp:lastModifiedBy></cp:lastModifiedBy>",
                    body,
                )
            elif info.filename == "xl/workbook.xml":
                body = re.sub(
                    rb"<mc:AlternateContent[^>]*><mc:Choice[^>]*><x15ac:absPath[^>]*/>"
                    rb"</mc:Choice></mc:AlternateContent>",
                    b"",
                    body,
                )
            members.append((info.filename, body))
    write_zip(out, members)
    print("wrote", out)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--scrub":
        scrub(sys.argv[2])
    else:
        generate()
