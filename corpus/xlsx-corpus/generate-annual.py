#!/usr/bin/env python3
# annual-charts.xlsx — the "Paged Annual" showcase workbook: ONE chart of
# EVERY curated ChartKind (spec §8.4; sheet-chart/src/model.rs).
#
# Sibling of generate.py and built the same way: stdlib only (zipfile +
# hand-authored XML strings), fixed zip member order + the fixed 1980
# timestamp, so re-running is byte-stable. The editor showcase copies the
# output verbatim (apps/canvas/tests/showcase/assets/annual-charts.xlsx);
# regenerate here, then re-copy.
#
#     python3 corpus/xlsx-corpus/generate-annual.py
#
# Shape: a `Data` sheet with the annual's quarterly circulation series, and
# a `Charts` sheet whose ONE drawing anchors TEN graphicFrames — chart1.xml
# through chart10.xml, one per curated kind, in this order (the drawing
# rels' document order, which `XlsxDocument::open` preserves):
#
#     1 column          c:barChart  barDir=col  grouping=clustered
#     2 bar             c:barChart  barDir=bar  grouping=clustered
#     3 stacked column  c:barChart  barDir=col  grouping=stacked
#     4 stacked bar     c:barChart  barDir=bar  grouping=stacked
#     5 line            c:lineChart
#     6 area            c:areaChart
#     7 pie             c:pieChart
#     8 donut           c:doughnutChart (holeSize 50)
#     9 scatter         c:scatterChart (c:xVal + c:yVal per series)
#    10 radar           c:radarChart
#
# These are EXACTLY the encodings sheet-xlsx/src/parts/chart.rs reads: the
# kind comes from the plot-area child element, bar-vs-column from c:barDir,
# the stacked variants from c:grouping val="stacked" (honoured only inside
# c:barChart), and donut is its own element (c:doughnutChart), not a pie
# attribute. Conformance: sheet-conformance/tests/chart.rs::
# sheet_chart_xlsx_part_kind_battery.
#
# Content is self-authored for the Paged Annual (print-shop circulation
# figures); no third-party material.

import os
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
FIXED_DATE = (1980, 1, 1, 0, 0, 0)

XML_DECL = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
NS_MAIN = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
NS_R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
NS_CT = "http://schemas.openxmlformats.org/package/2006/content-types"
NS_REL = "http://schemas.openxmlformats.org/package/2006/relationships"
NS_C = "http://schemas.openxmlformats.org/drawingml/2006/chart"
NS_A = "http://schemas.openxmlformats.org/drawingml/2006/main"
NS_XDR = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"

CT_WORKBOOK = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"
CT_WORKSHEET = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"
CT_RELS = "application/vnd.openxmlformats-package.relationships+xml"
CT_XML = "application/xml"
CT_DRAWING = "application/vnd.openxmlformats-officedocument.drawing+xml"
CT_CHART = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml"

RT_OFFICE_DOC = NS_R + "/officeDocument"
RT_WORKSHEET = NS_R + "/worksheet"
RT_DRAWING = NS_R + "/drawing"
RT_CHART = NS_R + "/chart"

# The annual's data: quarterly print + digital circulation, plus the press
# floor's spoilage-rate vs rerun-count pairs (the scatter's X/Y).
QUARTERS = ["Q1", "Q2", "Q3", "Q4"]
PRINT_RUN = [1840, 2110, 1975, 2390]
DIGITAL = [620, 880, 1210, 1585]
SPOILAGE = [3.2, 4.1, 2.8, 5.0]
RERUNS = [12, 19, 9, 24]

# Series fills — the annual's plate colours (same family as showcase-mark).
COLOR_PRINT = "1C3F94"
COLOR_DIGITAL = "D94F2B"


def write_zip(path, members):
    full = os.path.join(HERE, path)
    with zipfile.ZipFile(full, "w", zipfile.ZIP_DEFLATED) as z:
        for name, body in members:
            info = zipfile.ZipInfo(name, date_time=FIXED_DATE)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, body)
    print("wrote", path)


def rels(pairs):
    s = XML_DECL + f'<Relationships xmlns="{NS_REL}">'
    for rid, ty, tgt in pairs:
        s += f'<Relationship Id="{rid}" Type="{ty}" Target="{tgt}"/>'
    s += "</Relationships>"
    return s


def data_sheet():
    def srow(r, label, *nums):
        s = f'<row r="{r}"><c r="A{r}" t="inlineStr"><is><t>{label}</t></is></c>'
        for i, v in enumerate(nums):
            col = chr(ord("B") + i)
            s += f'<c r="{col}{r}" t="n"><v>{v}</v></c>'
        return s + "</row>"

    rows = (
        '<row r="1">'
        '<c r="A1" t="inlineStr"><is><t>Quarter</t></is></c>'
        '<c r="B1" t="inlineStr"><is><t>Print</t></is></c>'
        '<c r="C1" t="inlineStr"><is><t>Digital</t></is></c>'
        '<c r="D1" t="inlineStr"><is><t>Spoilage</t></is></c>'
        '<c r="E1" t="inlineStr"><is><t>Reruns</t></is></c>'
        "</row>"
    )
    for i, q in enumerate(QUARTERS):
        rows += srow(i + 2, q, PRINT_RUN[i], DIGITAL[i], SPOILAGE[i], RERUNS[i])
    return (
        XML_DECL
        + f'<worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}">'
        + '<dimension ref="A1:E5"/>'
        + "<sheetData>" + rows + "</sheetData>"
        + "</worksheet>"
    )


def charts_sheet():
    rows = (
        '<row r="1"><c r="A1" t="inlineStr">'
        "<is><t>Annual circulation charts</t></is></c></row>"
    )
    return (
        XML_DECL
        + f'<worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}">'
        + '<dimension ref="A1:A1"/>'
        + "<sheetData>" + rows + "</sheetData>"
        + '<drawing r:id="rId1"/>'
        + "</worksheet>"
    )


def drawing(n_charts):
    s = XML_DECL + f'<xdr:wsDr xmlns:xdr="{NS_XDR}" xmlns:a="{NS_A}" xmlns:r="{NS_R}">'
    for i in range(n_charts):
        top = 2 + i * 16
        s += (
            "<xdr:twoCellAnchor>"
            f"<xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff>"
            f"<xdr:row>{top}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>"
            f"<xdr:to><xdr:col>9</xdr:col><xdr:colOff>0</xdr:colOff>"
            f"<xdr:row>{top + 14}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>"
            "<xdr:graphicFrame><xdr:nvGraphicFramePr>"
            f'<xdr:cNvPr id="{i + 2}" name="Chart {i + 1}"/><xdr:cNvGraphicFramePr/>'
            "</xdr:nvGraphicFramePr>"
            '<xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm>'
            f'<a:graphic><a:graphicData uri="{NS_C}">'
            f'<c:chart xmlns:c="{NS_C}" r:id="rId{i + 1}"/>'
            "</a:graphicData></a:graphic></xdr:graphicFrame>"
            "<xdr:clientData/></xdr:twoCellAnchor>"
        )
    return s + "</xdr:wsDr>"


def num_ref(col, values, fmt="General"):
    s = f"<c:numRef><c:f>Data!${col}$2:${col}$5</c:f>"
    s += f'<c:numCache><c:formatCode>{fmt}</c:formatCode><c:ptCount val="{len(values)}"/>'
    for i, v in enumerate(values):
        s += f'<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>'
    return s + "</c:numCache></c:numRef>"


def cat_ref():
    s = "<c:strRef><c:f>Data!$A$2:$A$5</c:f>"
    s += f'<c:strCache><c:ptCount val="{len(QUARTERS)}"/>'
    for i, q in enumerate(QUARTERS):
        s += f'<c:pt idx="{i}"><c:v>{q}</c:v></c:pt>'
    return s + "</c:strCache></c:strRef>"


def ser(idx, name_cell, name, col, values, color, with_cat=True):
    s = f'<c:ser><c:idx val="{idx}"/><c:order val="{idx}"/>'
    s += f"<c:tx><c:strRef><c:f>Data!${name_cell}$1</c:f>"
    s += f'<c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>{name}</c:v></c:pt>'
    s += "</c:strCache></c:strRef></c:tx>"
    s += f'<c:spPr><a:solidFill><a:srgbClr val="{color}"/></a:solidFill></c:spPr>'
    if with_cat:
        s += "<c:cat>" + cat_ref() + "</c:cat>"
    s += "<c:val>" + num_ref(col, values) + "</c:val>"
    return s + "</c:ser>"


def both_series():
    return ser(0, "B", "Print", "B", PRINT_RUN, COLOR_PRINT) + ser(
        1, "C", "Digital", "C", DIGITAL, COLOR_DIGITAL
    )


def chart(title, plot_area):
    return (
        XML_DECL
        + f'<c:chartSpace xmlns:c="{NS_C}" xmlns:a="{NS_A}" xmlns:r="{NS_R}">'
        + "<c:chart>"
        + f"<c:title><c:tx><c:rich><a:p><a:r><a:t>{title}</a:t></a:r></a:p>"
        + "</c:rich></c:tx></c:title>"
        + "<c:plotArea>" + plot_area + "</c:plotArea>"
        + '<c:legend><c:legendPos val="r"/></c:legend>'
        + "</c:chart></c:chartSpace>"
    )


def bar_chart(bar_dir, grouping):
    return (
        f'<c:barChart><c:barDir val="{bar_dir}"/><c:grouping val="{grouping}"/>'
        + both_series()
        + '<c:axId val="1"/><c:axId val="2"/></c:barChart>'
    )


def scatter_ser():
    s = '<c:ser><c:idx val="0"/><c:order val="0"/>'
    s += "<c:tx><c:strRef><c:f>Data!$D$1</c:f>"
    s += '<c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>Spoilage</c:v></c:pt>'
    s += "</c:strCache></c:strRef></c:tx>"
    s += f'<c:spPr><a:solidFill><a:srgbClr val="{COLOR_PRINT}"/></a:solidFill></c:spPr>'
    s += "<c:xVal>" + num_ref("D", SPOILAGE, "0.0") + "</c:xVal>"
    s += "<c:yVal>" + num_ref("E", RERUNS) + "</c:yVal>"
    return s + "</c:ser>"


def all_charts():
    """The ten chart parts, in ChartKind battery order."""
    return [
        chart("Circulation by quarter (column)", bar_chart("col", "clustered")),
        chart("Circulation by quarter (bar)", bar_chart("bar", "clustered")),
        chart("Circulation, stacked columns", bar_chart("col", "stacked")),
        chart("Circulation, stacked bars", bar_chart("bar", "stacked")),
        chart(
            "Circulation trend (line)",
            "<c:lineChart>" + both_series()
            + '<c:axId val="1"/><c:axId val="2"/></c:lineChart>',
        ),
        chart(
            "Circulation volume (area)",
            "<c:areaChart>" + both_series()
            + '<c:axId val="1"/><c:axId val="2"/></c:areaChart>',
        ),
        chart(
            "Print share by quarter (pie)",
            "<c:pieChart>"
            + ser(0, "B", "Print", "B", PRINT_RUN, COLOR_PRINT)
            + "</c:pieChart>",
        ),
        chart(
            "Digital share by quarter (donut)",
            "<c:doughnutChart>"
            + ser(0, "C", "Digital", "C", DIGITAL, COLOR_DIGITAL)
            + '<c:holeSize val="50"/></c:doughnutChart>',
        ),
        chart(
            "Spoilage vs reruns (scatter)",
            '<c:scatterChart><c:scatterStyle val="marker"/>' + scatter_ser()
            + '<c:axId val="1"/><c:axId val="2"/></c:scatterChart>',
        ),
        chart(
            "Quarterly profile (radar)",
            '<c:radarChart><c:radarStyle val="standard"/>' + both_series()
            + '<c:axId val="1"/><c:axId val="2"/></c:radarChart>',
        ),
    ]


def main():
    charts = all_charts()
    overrides = [
        ("/xl/workbook.xml", CT_WORKBOOK),
        ("/xl/worksheets/sheet1.xml", CT_WORKSHEET),
        ("/xl/worksheets/sheet2.xml", CT_WORKSHEET),
        ("/xl/drawings/drawing1.xml", CT_DRAWING),
    ] + [(f"/xl/charts/chart{i + 1}.xml", CT_CHART) for i in range(len(charts))]

    ct = XML_DECL + f'<Types xmlns="{NS_CT}">'
    ct += f'<Default Extension="rels" ContentType="{CT_RELS}"/>'
    ct += f'<Default Extension="xml" ContentType="{CT_XML}"/>'
    for pn, t in overrides:
        ct += f'<Override PartName="{pn}" ContentType="{t}"/>'
    ct += "</Types>"

    workbook = (
        XML_DECL
        + f'<workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets>'
        + '<sheet name="Data" sheetId="1" r:id="rId1"/>'
        + '<sheet name="Charts" sheetId="2" r:id="rId2"/>'
        + "</sheets></workbook>"
    )

    members = [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", rels([("rId1", RT_OFFICE_DOC, "xl/workbook.xml")])),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", rels([
            ("rId1", RT_WORKSHEET, "worksheets/sheet1.xml"),
            ("rId2", RT_WORKSHEET, "worksheets/sheet2.xml"),
        ])),
        ("xl/worksheets/sheet1.xml", data_sheet()),
        ("xl/worksheets/sheet2.xml", charts_sheet()),
        ("xl/worksheets/_rels/sheet2.xml.rels", rels([
            ("rId1", RT_DRAWING, "../drawings/drawing1.xml"),
        ])),
        ("xl/drawings/drawing1.xml", drawing(len(charts))),
        ("xl/drawings/_rels/drawing1.xml.rels", rels([
            (f"rId{i + 1}", RT_CHART, f"../charts/chart{i + 1}.xml")
            for i in range(len(charts))
        ])),
    ] + [
        (f"xl/charts/chart{i + 1}.xml", body) for i, body in enumerate(charts)
    ]
    write_zip("annual-charts.xlsx", members)


if __name__ == "__main__":
    main()
