from __future__ import annotations

from pathlib import Path
from textwrap import dedent

from docx import Document
from docx.enum.section import WD_SECTION_START
from docx.enum.table import WD_ALIGN_VERTICAL, WD_TABLE_ALIGNMENT
from docx.enum.text import WD_ALIGN_PARAGRAPH, WD_BREAK
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.shared import Inches, Pt, RGBColor


ROOT = Path(__file__).resolve().parent
OUT = ROOT / "Strivers_A2Z_DSA_Handbook.docx"

PAGE_WIDTH_DXA = 9360
TABLE_INDENT_DXA = 120

NAVY = RGBColor(11, 37, 69)
BLUE = RGBColor(46, 116, 181)
TEAL = RGBColor(0, 121, 107)
GOLD = RGBColor(122, 90, 0)
RED = RGBColor(155, 28, 28)
GRAY = RGBColor(85, 85, 85)
LIGHT_BLUE = "E8EEF5"
LIGHT_GRAY = "F2F4F7"
LIGHT_TEAL = "E8F4F2"
LIGHT_GOLD = "FFF7E0"
LIGHT_RED = "FDECEC"
INK = RGBColor(33, 37, 41)


def clean(text: str) -> str:
    return " ".join(text.strip().split())


def set_run_font(run, name: str = "Calibri", size: float | None = None, color: RGBColor | None = None,
                 bold: bool | None = None, italic: bool | None = None) -> None:
    run.font.name = name
    if run._element.rPr is None:
        run._element.get_or_add_rPr()
    run._element.rPr.rFonts.set(qn("w:ascii"), name)
    run._element.rPr.rFonts.set(qn("w:hAnsi"), name)
    if size is not None:
        run.font.size = Pt(size)
    if color is not None:
        run.font.color.rgb = color
    if bold is not None:
        run.bold = bold
    if italic is not None:
        run.italic = italic


def shade(element, fill: str) -> None:
    tc_pr = element._tc.get_or_add_tcPr() if hasattr(element, "_tc") else element._p.get_or_add_pPr()
    shd = OxmlElement("w:shd")
    shd.set(qn("w:fill"), fill)
    tc_pr.append(shd)


def set_cell_shading(cell, fill: str) -> None:
    tc_pr = cell._tc.get_or_add_tcPr()
    shd = tc_pr.find(qn("w:shd"))
    if shd is None:
        shd = OxmlElement("w:shd")
        tc_pr.append(shd)
    shd.set(qn("w:fill"), fill)


def set_cell_margins(cell, top: int = 80, bottom: int = 80, start: int = 120, end: int = 120) -> None:
    tc_pr = cell._tc.get_or_add_tcPr()
    tc_mar = tc_pr.find(qn("w:tcMar"))
    if tc_mar is None:
        tc_mar = OxmlElement("w:tcMar")
        tc_pr.append(tc_mar)
    for name, value in (("top", top), ("bottom", bottom), ("start", start), ("end", end)):
        node = tc_mar.find(qn(f"w:{name}"))
        if node is None:
            node = OxmlElement(f"w:{name}")
            tc_mar.append(node)
        node.set(qn("w:w"), str(value))
        node.set(qn("w:type"), "dxa")


def set_cell_border(cell, color: str = "D7DBE2", size: str = "6") -> None:
    tc_pr = cell._tc.get_or_add_tcPr()
    borders = tc_pr.find(qn("w:tcBorders"))
    if borders is None:
        borders = OxmlElement("w:tcBorders")
        tc_pr.append(borders)
    for edge in ("top", "left", "bottom", "right", "insideH", "insideV"):
        tag = qn(f"w:{edge}")
        node = borders.find(tag)
        if node is None:
            node = OxmlElement(f"w:{edge}")
            borders.append(node)
        node.set(qn("w:val"), "single")
        node.set(qn("w:sz"), size)
        node.set(qn("w:space"), "0")
        node.set(qn("w:color"), color)


def set_table_geometry(table, widths_dxa: list[int], indent_dxa: int = TABLE_INDENT_DXA) -> None:
    table.alignment = WD_TABLE_ALIGNMENT.LEFT
    table.autofit = False
    tbl = table._tbl
    tbl_pr = tbl.tblPr

    tbl_w = tbl_pr.find(qn("w:tblW"))
    if tbl_w is None:
        tbl_w = OxmlElement("w:tblW")
        tbl_pr.append(tbl_w)
    tbl_w.set(qn("w:w"), str(sum(widths_dxa)))
    tbl_w.set(qn("w:type"), "dxa")

    tbl_ind = tbl_pr.find(qn("w:tblInd"))
    if tbl_ind is None:
        tbl_ind = OxmlElement("w:tblInd")
        tbl_pr.append(tbl_ind)
    tbl_ind.set(qn("w:w"), str(indent_dxa))
    tbl_ind.set(qn("w:type"), "dxa")

    layout = tbl_pr.find(qn("w:tblLayout"))
    if layout is None:
        layout = OxmlElement("w:tblLayout")
        tbl_pr.append(layout)
    layout.set(qn("w:type"), "fixed")

    grid = tbl.tblGrid
    for child in list(grid):
        grid.remove(child)
    for width in widths_dxa:
        col = OxmlElement("w:gridCol")
        col.set(qn("w:w"), str(width))
        grid.append(col)

    for row in table.rows:
        for idx, cell in enumerate(row.cells):
            width = widths_dxa[min(idx, len(widths_dxa) - 1)]
            cell.width = Inches(width / 1440)
            tc_pr = cell._tc.get_or_add_tcPr()
            tc_w = tc_pr.find(qn("w:tcW"))
            if tc_w is None:
                tc_w = OxmlElement("w:tcW")
                tc_pr.append(tc_w)
            tc_w.set(qn("w:w"), str(width))
            tc_w.set(qn("w:type"), "dxa")
            cell.vertical_alignment = WD_ALIGN_VERTICAL.CENTER
            set_cell_margins(cell)
            set_cell_border(cell)


def mark_header_row(row) -> None:
    tr_pr = row._tr.get_or_add_trPr()
    tbl_header = tr_pr.find(qn("w:tblHeader"))
    if tbl_header is None:
        tbl_header = OxmlElement("w:tblHeader")
        tr_pr.append(tbl_header)
    tbl_header.set(qn("w:val"), "true")


def add_field(paragraph, instr: str, placeholder: str = "") -> None:
    run = paragraph.add_run()
    fld_begin = OxmlElement("w:fldChar")
    fld_begin.set(qn("w:fldCharType"), "begin")
    run._r.append(fld_begin)

    instr_run = paragraph.add_run()
    instr_text = OxmlElement("w:instrText")
    instr_text.set(qn("xml:space"), "preserve")
    instr_text.text = instr
    instr_run._r.append(instr_text)

    sep_run = paragraph.add_run()
    fld_sep = OxmlElement("w:fldChar")
    fld_sep.set(qn("w:fldCharType"), "separate")
    sep_run._r.append(fld_sep)

    if placeholder:
        paragraph.add_run(placeholder)

    end_run = paragraph.add_run()
    fld_end = OxmlElement("w:fldChar")
    fld_end.set(qn("w:fldCharType"), "end")
    end_run._r.append(fld_end)


def add_page_number(paragraph) -> None:
    paragraph.add_run("Page ")
    add_field(paragraph, "PAGE", "1")


def add_toc(doc: Document) -> None:
    h = doc.add_paragraph(style="Heading 1")
    h.add_run("Table of Contents")
    p = doc.add_paragraph()
    p.paragraph_format.space_after = Pt(18)
    add_field(p, r'TOC \o "1-2" \h \z \u', "Update fields in Word to populate the table of contents.")


def add_para(doc: Document, text: str, style: str = "Normal", before: float | None = None,
             after: float | None = None, bold_first: str | None = None) -> None:
    p = doc.add_paragraph(style=style)
    if before is not None:
        p.paragraph_format.space_before = Pt(before)
    if after is not None:
        p.paragraph_format.space_after = Pt(after)
    if bold_first and text.startswith(bold_first):
        r = p.add_run(bold_first)
        r.bold = True
        p.add_run(text[len(bold_first):])
    else:
        p.add_run(text)


def add_bullets(doc: Document, items: list[str]) -> None:
    for item in items:
        p = doc.add_paragraph(style="List Bullet")
        p.paragraph_format.left_indent = Inches(0.375)
        p.paragraph_format.first_line_indent = Inches(-0.187)
        p.paragraph_format.space_after = Pt(3)
        p.add_run(item)


def add_numbers(doc: Document, items: list[str]) -> None:
    for idx, item in enumerate(items, start=1):
        p = doc.add_paragraph()
        p.paragraph_format.left_indent = Inches(0.18)
        p.paragraph_format.space_after = Pt(3)
        r = p.add_run(f"Step {idx}: ")
        set_run_font(r, bold=True, color=NAVY)
        p.add_run(item)


def add_subhead(doc: Document, text: str) -> None:
    p = doc.add_paragraph(style="Topic Subhead")
    p.add_run(text)


def add_callout(doc: Document, label: str, text: str, kind: str = "note") -> None:
    fill = {"note": LIGHT_BLUE, "tip": LIGHT_TEAL, "warn": LIGHT_GOLD, "risk": LIGHT_RED}.get(kind, LIGHT_BLUE)
    table = doc.add_table(rows=1, cols=1)
    set_table_geometry(table, [PAGE_WIDTH_DXA], indent_dxa=TABLE_INDENT_DXA)
    cell = table.cell(0, 0)
    set_cell_shading(cell, fill)
    p = cell.paragraphs[0]
    p.paragraph_format.space_after = Pt(0)
    p.paragraph_format.line_spacing = 1.15
    r = p.add_run(label + ": ")
    set_run_font(r, bold=True, color=NAVY)
    p.add_run(text)
    doc.add_paragraph().paragraph_format.space_after = Pt(2)


def add_code(doc: Document, code: str) -> None:
    table = doc.add_table(rows=1, cols=1)
    set_table_geometry(table, [PAGE_WIDTH_DXA], indent_dxa=TABLE_INDENT_DXA)
    cell = table.cell(0, 0)
    set_cell_shading(cell, "F7F8FA")
    set_cell_border(cell, "D4D9E1", "4")
    p = cell.paragraphs[0]
    p.paragraph_format.space_before = Pt(0)
    p.paragraph_format.space_after = Pt(0)
    p.paragraph_format.line_spacing = 1.0
    lines = dedent(code).strip("\n").splitlines()
    for i, line in enumerate(lines):
        run = p.add_run(line.rstrip())
        set_run_font(run, "Consolas", 8.5, RGBColor(31, 41, 55))
        if i != len(lines) - 1:
            run.add_break()
    doc.add_paragraph().paragraph_format.space_after = Pt(2)


def add_table(doc: Document, headers: list[str], rows: list[list[str]], widths: list[int],
              header_fill: str = LIGHT_BLUE) -> None:
    table = doc.add_table(rows=1, cols=len(headers))
    set_table_geometry(table, widths, indent_dxa=TABLE_INDENT_DXA)
    mark_header_row(table.rows[0])
    hdr = table.rows[0].cells
    for idx, text in enumerate(headers):
        set_cell_shading(hdr[idx], header_fill)
        p = hdr[idx].paragraphs[0]
        p.paragraph_format.space_after = Pt(0)
        r = p.add_run(text)
        set_run_font(r, bold=True, color=NAVY)
    for row in rows:
        cells = table.add_row().cells
        for idx, text in enumerate(row):
            p = cells[idx].paragraphs[0]
            p.paragraph_format.space_after = Pt(0)
            p.paragraph_format.line_spacing = 1.05
            run = p.add_run(text)
            set_run_font(run, size=9.2, color=INK)
    set_table_geometry(table, widths, indent_dxa=TABLE_INDENT_DXA)
    doc.add_paragraph().paragraph_format.space_after = Pt(4)


def setup_styles(doc: Document) -> None:
    section = doc.sections[0]
    section.page_width = Inches(8.5)
    section.page_height = Inches(11)
    section.top_margin = Inches(1)
    section.bottom_margin = Inches(1)
    section.left_margin = Inches(1)
    section.right_margin = Inches(1)
    section.header_distance = Inches(0.492)
    section.footer_distance = Inches(0.492)
    section.different_first_page_header_footer = True

    styles = doc.styles
    normal = styles["Normal"]
    normal.font.name = "Calibri"
    normal._element.rPr.rFonts.set(qn("w:ascii"), "Calibri")
    normal._element.rPr.rFonts.set(qn("w:hAnsi"), "Calibri")
    normal.font.size = Pt(10.5)
    normal.font.color.rgb = INK
    normal.paragraph_format.space_after = Pt(5)
    normal.paragraph_format.line_spacing = 1.18

    for name, size, color, before, after in [
        ("Heading 1", 16, BLUE, 16, 8),
        ("Heading 2", 13, BLUE, 12, 6),
        ("Heading 3", 12, RGBColor(31, 77, 120), 8, 4),
    ]:
        style = styles[name]
        style.font.name = "Calibri"
        style._element.rPr.rFonts.set(qn("w:ascii"), "Calibri")
        style._element.rPr.rFonts.set(qn("w:hAnsi"), "Calibri")
        style.font.size = Pt(size)
        style.font.bold = True
        style.font.color.rgb = color
        style.paragraph_format.space_before = Pt(before)
        style.paragraph_format.space_after = Pt(after)
        style.paragraph_format.keep_with_next = True

    sub = styles.add_style("Topic Subhead", 1)
    sub.base_style = normal
    sub.font.name = "Calibri"
    sub._element.rPr.rFonts.set(qn("w:ascii"), "Calibri")
    sub._element.rPr.rFonts.set(qn("w:hAnsi"), "Calibri")
    sub.font.size = Pt(11.5)
    sub.font.bold = True
    sub.font.color.rgb = NAVY
    sub.paragraph_format.space_before = Pt(7)
    sub.paragraph_format.space_after = Pt(3)
    sub.paragraph_format.keep_with_next = True

    small = styles.add_style("Small Note", 1)
    small.base_style = normal
    small.font.size = Pt(9)
    small.font.color.rgb = GRAY
    small.paragraph_format.space_after = Pt(4)

    title = styles.add_style("Cover Title", 1)
    title.font.name = "Calibri Light"
    title._element.rPr.rFonts.set(qn("w:ascii"), "Calibri Light")
    title._element.rPr.rFonts.set(qn("w:hAnsi"), "Calibri Light")
    title.font.size = Pt(30)
    title.font.color.rgb = NAVY
    title.font.bold = True
    title.paragraph_format.alignment = WD_ALIGN_PARAGRAPH.CENTER
    title.paragraph_format.space_after = Pt(8)

    subtitle = styles.add_style("Cover Subtitle", 1)
    subtitle.font.name = "Calibri"
    subtitle._element.rPr.rFonts.set(qn("w:ascii"), "Calibri")
    subtitle._element.rPr.rFonts.set(qn("w:hAnsi"), "Calibri")
    subtitle.font.size = Pt(13.5)
    subtitle.font.color.rgb = GRAY
    subtitle.paragraph_format.alignment = WD_ALIGN_PARAGRAPH.CENTER
    subtitle.paragraph_format.space_after = Pt(7)

    for style_name in ("List Bullet", "List Number"):
        style = styles[style_name]
        style.font.name = "Calibri"
        style._element.rPr.rFonts.set(qn("w:ascii"), "Calibri")
        style._element.rPr.rFonts.set(qn("w:hAnsi"), "Calibri")
        style.font.size = Pt(10.5)
        style.paragraph_format.space_after = Pt(3)
        style.paragraph_format.line_spacing = 1.15


def setup_header_footer(doc: Document) -> None:
    for section in doc.sections:
        header = section.header
        hp = header.paragraphs[0]
        hp.text = ""
        hp.alignment = WD_ALIGN_PARAGRAPH.LEFT
        r = hp.add_run("Striver's A2Z DSA Handbook")
        set_run_font(r, size=9, color=GRAY, bold=True)

        footer = section.footer
        fp = footer.paragraphs[0]
        fp.text = ""
        fp.alignment = WD_ALIGN_PARAGRAPH.RIGHT
        for run in fp.runs:
            set_run_font(run, size=9, color=GRAY)
        add_page_number(fp)


def add_cover(doc: Document) -> None:
    for _ in range(5):
        doc.add_paragraph()
    kicker = doc.add_paragraph()
    kicker.alignment = WD_ALIGN_PARAGRAPH.CENTER
    r = kicker.add_run("INTERVIEW PREPARATION HANDBOOK")
    set_run_font(r, size=10, color=GOLD, bold=True)

    p = doc.add_paragraph(style="Cover Title")
    p.add_run("Striver's A2Z DSA Handbook")

    p = doc.add_paragraph(style="Cover Subtitle")
    p.add_run("The Complete Interview Preparation Guide for Product-Based Companies")

    p = doc.add_paragraph(style="Cover Subtitle")
    r = p.add_run("Master Data Structures and Algorithms with Interview Patterns, Complexity Analysis, Java Implementations, and FAANG-Level Problem Solving")
    r.italic = True

    doc.add_paragraph()
    add_table(
        doc,
        ["Audience", "Language", "Scope", "Edition"],
        [[
            "Intermediate Java programmers and college students",
            "Java-first explanations and templates",
            "A2Z DSA topics, pattern handbooks, strategy, and appendices",
            "2026 interview edition"
        ]],
        [2380, 1700, 3600, 1680],
        header_fill=LIGHT_TEAL,
    )

    add_callout(
        doc,
        "Scope note",
        "This is an original educational handbook inspired by the public Striver A2Z roadmap. It does not reproduce official solutions; it teaches the concepts, patterns, and interview reasoning needed to solve the sheet independently.",
        "note",
    )
    doc.add_page_break()


def add_front_matter(doc: Document) -> None:
    doc.add_heading("How to Use This Handbook", level=1)
    add_para(doc, "The fastest way to use this book is to study by pattern, then verify by solving. Each topic chapter follows the same interview loop: understand the idea, recognize the pattern, implement the template, dry-run by hand, and then practice problems in rising difficulty.")
    add_table(
        doc,
        ["Study pass", "Goal", "What to do"],
        [
            ["Pass 1", "Build vocabulary", "Read Concept, Why it exists, Analogy, and Diagram. Do not memorize code yet."],
            ["Pass 2", "Build implementation fluency", "Rewrite the Java template from memory, then dry-run it on a small input."],
            ["Pass 3", "Build interview judgment", "Focus on tricks, pattern recognition, and how constraints guide the solution."],
            ["Pass 4", "Build speed", "Solve Easy to Medium first, then revisit Hard problems with a notebook of failed ideas."],
        ],
        [1600, 2200, 5560],
    )
    add_callout(doc, "Interview note", "A strong candidate explains invariants and tradeoffs while coding. The code matters, but the reasoning around it is what interviewers score.", "tip")
    add_para(doc, "Recommended pace: finish the Foundations and Core Data Structures first, then cycle through Binary Search, Graphs, Dynamic Programming, and Advanced Structures. Use the 30/60/90-day plans near the end depending on your deadline.")
    doc.add_page_break()
    add_toc(doc)
    doc.add_page_break()


COMMON_COMPLEXITIES = {
    "Programming Basics": [["Input parsing", "O(n)", "O(1)", "n tokens read once"], ["Array traversal", "O(n)", "O(1)", "Single pass"], ["Nested loops", "O(n*m)", "O(1)", "Depends on bounds"]],
    "Complexity Analysis": [["Single loop", "O(n)", "O(1)", "Linear scan"], ["Divide and conquer", "O(n log n)", "O(log n)", "Typical merge/quick recursion"], ["Hash lookup average", "O(1)", "O(n)", "Worst-case can degrade"]],
    "Mathematics": [["GCD", "O(log min(a,b))", "O(1)", "Euclid"], ["Sieve", "O(n log log n)", "O(n)", "Prime precomputation"], ["Fast power", "O(log n)", "O(1)", "Binary exponentiation"]],
    "Recursion": [["Recursive call tree", "Depends", "O(depth)", "Stack frames"], ["Backtracking", "O(branch^depth)", "O(depth)", "Search space"], ["Memoized recursion", "O(states*transition)", "O(states)", "Avoids repeats"]],
    "Hashing": [["Insert/find/delete", "O(1) avg", "O(n)", "HashMap/HashSet"], ["Ordered map", "O(log n)", "O(n)", "TreeMap"], ["Frequency count", "O(n)", "O(k)", "k distinct keys"]],
    "Sorting": [["Selection sort", "O(n^2)", "O(1)", "Educational"], ["Merge sort", "O(n log n)", "O(n)", "Stable"], ["Quick sort", "O(n log n) avg", "O(log n)", "Worst O(n^2)"]],
    "Arrays": [["Access", "O(1)", "O(1)", "Index arithmetic"], ["Search", "O(n)", "O(1)", "Unsorted"], ["Insert/delete middle", "O(n)", "O(1)", "Shifting"]],
    "Binary Search": [["Search sorted", "O(log n)", "O(1)", "Invariant halves space"], ["Lower bound", "O(log n)", "O(1)", "First true"], ["Answer search", "O(log range * check)", "O(1)", "Monotonic predicate"]],
    "Strings": [["Index scan", "O(n)", "O(1)", "Characters"], ["Substring naive", "O(n*m)", "O(1)", "May TLE"], ["KMP", "O(n+m)", "O(m)", "Prefix function"]],
    "Linked List": [["Access kth", "O(k)", "O(1)", "No random access"], ["Insert/delete known node", "O(1)", "O(1)", "Pointer change"], ["Reverse", "O(n)", "O(1)", "Three pointers"]],
    "Stack": [["Push/pop/top", "O(1)", "O(n)", "LIFO"], ["Expression eval", "O(n)", "O(n)", "Operators/values"], ["Monotonic stack", "O(n)", "O(n)", "Each item enters/exits once"]],
    "Queue": [["Enqueue/dequeue", "O(1)", "O(n)", "FIFO"], ["BFS", "O(V+E)", "O(V)", "Layer order"], ["Circular buffer", "O(1)", "O(n)", "Fixed capacity"]],
    "Sliding Window": [["Fixed window", "O(n)", "O(1)", "Add right/remove left"], ["Variable window", "O(n)", "O(k)", "Each pointer moves once"], ["Exactly K", "O(n)", "O(k)", "atMost(K)-atMost(K-1)"]],
    "Two Pointer": [["Opposite ends", "O(n)", "O(1)", "Sorted array"], ["Fast/slow", "O(n)", "O(1)", "Cycle/middle"], ["Merge", "O(n+m)", "O(1)", "Two sorted streams"]],
    "Greedy": [["Sort + choose", "O(n log n)", "O(1)", "Ordering dominates"], ["Heap greedy", "O(n log n)", "O(n)", "Repeated best choice"], ["Interval greedy", "O(n log n)", "O(1)", "Earliest finish"]],
    "Binary Trees": [["DFS traversal", "O(n)", "O(h)", "Recursion stack"], ["BFS traversal", "O(n)", "O(width)", "Queue"], ["Height/balance", "O(n)", "O(h)", "Postorder"]],
    "BST": [["Search/insert/delete", "O(h)", "O(h)", "h can be n"], ["Balanced search", "O(log n)", "O(log n)", "If height controlled"], ["Inorder", "O(n)", "O(h)", "Sorted order"]],
    "Heap": [["Push/pop", "O(log n)", "O(n)", "PriorityQueue"], ["Peek", "O(1)", "O(n)", "Best element"], ["Build heap", "O(n)", "O(n)", "Heapify"]],
    "Trie": [["Insert/search", "O(L)", "O(nodes)", "L word length"], ["Prefix query", "O(L)", "O(nodes)", "Autocomplete"], ["Bitwise trie", "O(bits)", "O(n*bits)", "XOR problems"]],
    "Graphs": [["DFS/BFS", "O(V+E)", "O(V)", "Adjacency list"], ["Matrix scan", "O(V^2)", "O(V^2)", "Dense graph"], ["Grid graph", "O(R*C)", "O(R*C)", "Cells as vertices"]],
    "Dynamic Programming": [["Memoization", "O(states*transition)", "O(states)", "Top-down"], ["Tabulation", "O(states*transition)", "O(states)", "Bottom-up"], ["Space optimized", "Same time", "Reduced", "Keep needed rows"]],
    "Bit Manipulation": [["Get/set/clear bit", "O(1)", "O(1)", "Constant"], ["Count set bits", "O(log n)", "O(1)", "Brian Kernighan"], ["XOR partition", "O(n)", "O(1)", "Pair cancellation"]],
    "Segment Tree": [["Build", "O(n)", "O(4n)", "Recursive tree"], ["Query/update", "O(log n)", "O(4n)", "Range aggregate"], ["Lazy update", "O(log n)", "O(4n)", "Deferred propagation"]],
    "Fenwick Tree": [["Build by updates", "O(n log n)", "O(n)", "Or O(n) variant"], ["Prefix query", "O(log n)", "O(n)", "lowbit"], ["Point update", "O(log n)", "O(n)", "Climb indexes"]],
    "Disjoint Set Union": [["Find", "Alpha(n)", "O(n)", "Path compression"], ["Union", "Alpha(n)", "O(n)", "By size/rank"], ["Kruskal", "O(E log E)", "O(V)", "Sort edges"]],
    "Backtracking": [["Generate subsets", "O(2^n)", "O(n)", "Include/exclude"], ["Permutations", "O(n!)", "O(n)", "Swap/used"], ["Constraint search", "Exponential", "O(depth)", "Prune early"]],
    "Binary Lifting": [["Preprocess", "O(n log n)", "O(n log n)", "Jump table"], ["kth ancestor", "O(log n)", "O(1)", "Bits of k"], ["LCA", "O(log n)", "O(1)", "Lift depths"]],
    "Euler Tour": [["DFS tour", "O(n)", "O(n)", "tin/tout"], ["Subtree query", "O(log n)", "O(n)", "With BIT/segment tree"], ["Ancestor check", "O(1)", "O(n)", "tin/tout containment"]],
    "Sparse Table": [["Build", "O(n log n)", "O(n log n)", "Idempotent ops"], ["RMQ query", "O(1)", "O(1)", "Overlapping blocks"], ["Non-idempotent", "O(log n)", "O(1)", "If decomposed"]],
    "Meet-in-the-Middle": [["Split enumerate", "O(2^(n/2))", "O(2^(n/2))", "Two halves"], ["Sort one half", "O(m log m)", "O(m)", "Binary search"], ["Combine", "O(m log m)", "O(m)", "m=2^(n/2)"]],
}


def base_complexities(name: str) -> list[list[str]]:
    if name in COMMON_COMPLEXITIES:
        return COMMON_COMPLEXITIES[name]
    for key, value in COMMON_COMPLEXITIES.items():
        if key in name or name in key:
            return value
    return [["Core operation", "Usually O(log n) to O(n)", "Depends", "Check constraints"], ["Preprocessing", "Often O(n log n)", "O(n)", "For repeated queries"], ["Query", "From O(1) to O(log n)", "O(1)", "Depends on structure"]]


CODE = {
    "Programming Basics": r"""
import java.io.*;
import java.util.*;

public class Main {
    static class FastScanner {
        private final InputStream in = System.in;
        private final byte[] buffer = new byte[1 << 16];
        private int ptr = 0, len = 0;
        int read() throws IOException {
            if (ptr >= len) { len = in.read(buffer); ptr = 0; }
            return len <= 0 ? -1 : buffer[ptr++];
        }
        int nextInt() throws IOException {
            int c, sign = 1, val = 0;
            do { c = read(); } while (c <= ' ' && c != -1);
            if (c == '-') { sign = -1; c = read(); }
            while (c > ' ') { val = val * 10 + c - '0'; c = read(); }
            return val * sign;
        }
    }
    public static void main(String[] args) throws Exception {
        FastScanner fs = new FastScanner();
        int n = fs.nextInt();
        long sum = 0;
        for (int i = 0; i < n; i++) sum += fs.nextInt();
        System.out.println(sum);
    }
}
""",
    "Complexity Analysis": r"""
class ComplexityExamples {
    static long triangularWork(int n) {
        long work = 0;
        for (int i = 1; i <= n; i++) {
            for (int j = 1; j <= i; j++) {
                work++;
            }
        }
        return work; // n * (n + 1) / 2 -> O(n^2)
    }

    static int binarySearchIterations(int n) {
        int count = 0;
        while (n > 1) {
            n /= 2;
            count++;
        }
        return count; // about log2(n)
    }
}
""",
    "Mathematics": r"""
class MathToolkit {
    static long gcd(long a, long b) {
        while (b != 0) {
            long t = a % b;
            a = b;
            b = t;
        }
        return Math.abs(a);
    }

    static long modPow(long a, long e, long mod) {
        long ans = 1 % mod;
        while (e > 0) {
            if ((e & 1) == 1) ans = (ans * a) % mod;
            a = (a * a) % mod;
            e >>= 1;
        }
        return ans;
    }
}
""",
    "Recursion": r"""
class RecursionTemplate {
    static void subsets(int[] a, int idx, java.util.List<Integer> path,
                        java.util.List<java.util.List<Integer>> ans) {
        if (idx == a.length) {
            ans.add(new java.util.ArrayList<>(path));
            return;
        }
        subsets(a, idx + 1, path, ans);       // do not take
        path.add(a[idx]);
        subsets(a, idx + 1, path, ans);       // take
        path.remove(path.size() - 1);         // undo
    }
}
""",
    "Hashing": r"""
import java.util.*;

class FrequencyCounter {
    static Map<Integer, Integer> frequency(int[] nums) {
        Map<Integer, Integer> freq = new HashMap<>();
        for (int x : nums) freq.put(x, freq.getOrDefault(x, 0) + 1);
        return freq;
    }

    static boolean hasTwoSum(int[] nums, int target) {
        Set<Integer> seen = new HashSet<>();
        for (int x : nums) {
            if (seen.contains(target - x)) return true;
            seen.add(x);
        }
        return false;
    }
}
""",
    "Sorting": r"""
class MergeSort {
    static void sort(int[] a) {
        int[] tmp = new int[a.length];
        mergeSort(a, 0, a.length - 1, tmp);
    }
    static void mergeSort(int[] a, int l, int r, int[] tmp) {
        if (l >= r) return;
        int m = l + (r - l) / 2;
        mergeSort(a, l, m, tmp);
        mergeSort(a, m + 1, r, tmp);
        int i = l, j = m + 1, k = l;
        while (i <= m && j <= r) tmp[k++] = a[i] <= a[j] ? a[i++] : a[j++];
        while (i <= m) tmp[k++] = a[i++];
        while (j <= r) tmp[k++] = a[j++];
        for (i = l; i <= r; i++) a[i] = tmp[i];
    }
}
""",
    "Arrays": r"""
class ArrayPatterns {
    static void rotateRight(int[] a, int k) {
        int n = a.length;
        k %= n;
        reverse(a, 0, n - 1);
        reverse(a, 0, k - 1);
        reverse(a, k, n - 1);
    }
    static void reverse(int[] a, int l, int r) {
        while (l < r) {
            int t = a[l]; a[l++] = a[r]; a[r--] = t;
        }
    }
}
""",
    "Prefix Sum": r"""
class PrefixSum {
    private final long[] pref;
    PrefixSum(int[] nums) {
        pref = new long[nums.length + 1];
        for (int i = 0; i < nums.length; i++) pref[i + 1] = pref[i] + nums[i];
    }
    long rangeSum(int left, int right) { // inclusive
        return pref[right + 1] - pref[left];
    }
}
""",
    "Difference Array": r"""
class DifferenceArray {
    static int[] applyRangeAdds(int n, int[][] queries) {
        int[] diff = new int[n + 1];
        for (int[] q : queries) {
            int l = q[0], r = q[1], val = q[2];
            diff[l] += val;
            if (r + 1 < n) diff[r + 1] -= val;
        }
        int[] ans = new int[n];
        int cur = 0;
        for (int i = 0; i < n; i++) {
            cur += diff[i];
            ans[i] = cur;
        }
        return ans;
    }
}
""",
    "Kadane": r"""
class Kadane {
    static int maxSubArray(int[] nums) {
        int best = nums[0], cur = nums[0];
        for (int i = 1; i < nums.length; i++) {
            cur = Math.max(nums[i], cur + nums[i]);
            best = Math.max(best, cur);
        }
        return best;
    }
}
""",
    "Binary Search": r"""
class BinarySearchPatterns {
    static int lowerBound(int[] a, int target) {
        int lo = 0, hi = a.length;
        while (lo < hi) {
            int mid = lo + (hi - lo) / 2;
            if (a[mid] >= target) hi = mid;
            else lo = mid + 1;
        }
        return lo;
    }
}
""",
    "Strings": r"""
class KMP {
    static int[] prefix(String p) {
        int[] pi = new int[p.length()];
        for (int i = 1; i < p.length(); i++) {
            int j = pi[i - 1];
            while (j > 0 && p.charAt(i) != p.charAt(j)) j = pi[j - 1];
            if (p.charAt(i) == p.charAt(j)) j++;
            pi[i] = j;
        }
        return pi;
    }
}
""",
    "Linked List": r"""
class LinkedListReverse {
    static class Node {
        int val;
        Node next;
        Node(int v) { val = v; }
    }
    static Node reverse(Node head) {
        Node prev = null, cur = head;
        while (cur != null) {
            Node next = cur.next;
            cur.next = prev;
            prev = cur;
            cur = next;
        }
        return prev;
    }
}
""",
    "Stack": r"""
import java.util.*;

class MinStack {
    private final Deque<Integer> st = new ArrayDeque<>();
    private final Deque<Integer> min = new ArrayDeque<>();
    void push(int x) {
        st.push(x);
        if (min.isEmpty() || x <= min.peek()) min.push(x);
    }
    int pop() {
        int x = st.pop();
        if (x == min.peek()) min.pop();
        return x;
    }
    int getMin() { return min.peek(); }
}
""",
    "Queue": r"""
class CircularQueue {
    private final int[] a;
    private int head = 0, tail = 0, size = 0;
    CircularQueue(int capacity) { a = new int[capacity]; }
    boolean offer(int x) {
        if (size == a.length) return false;
        a[tail] = x;
        tail = (tail + 1) % a.length;
        size++;
        return true;
    }
    int poll() {
        if (size == 0) throw new java.util.NoSuchElementException();
        int x = a[head];
        head = (head + 1) % a.length;
        size--;
        return x;
    }
}
""",
    "Monotonic Stack": r"""
import java.util.*;

class NextGreaterElement {
    static int[] nextGreater(int[] nums) {
        int n = nums.length;
        int[] ans = new int[n];
        Arrays.fill(ans, -1);
        Deque<Integer> st = new ArrayDeque<>(); // indexes with decreasing values
        for (int i = 0; i < n; i++) {
            while (!st.isEmpty() && nums[i] > nums[st.peek()]) ans[st.pop()] = nums[i];
            st.push(i);
        }
        return ans;
    }
}
""",
    "Monotonic Queue": r"""
import java.util.*;

class SlidingWindowMaximum {
    static int[] maxSlidingWindow(int[] nums, int k) {
        int[] ans = new int[nums.length - k + 1];
        Deque<Integer> dq = new ArrayDeque<>();
        for (int i = 0; i < nums.length; i++) {
            while (!dq.isEmpty() && dq.peekFirst() <= i - k) dq.pollFirst();
            while (!dq.isEmpty() && nums[dq.peekLast()] <= nums[i]) dq.pollLast();
            dq.offerLast(i);
            if (i >= k - 1) ans[i - k + 1] = nums[dq.peekFirst()];
        }
        return ans;
    }
}
""",
    "Sliding Window": r"""
import java.util.*;

class LongestAtMostKDistinct {
    static int longest(String s, int k) {
        Map<Character, Integer> count = new HashMap<>();
        int left = 0, best = 0;
        for (int right = 0; right < s.length(); right++) {
            count.put(s.charAt(right), count.getOrDefault(s.charAt(right), 0) + 1);
            while (count.size() > k) {
                char c = s.charAt(left++);
                count.put(c, count.get(c) - 1);
                if (count.get(c) == 0) count.remove(c);
            }
            best = Math.max(best, right - left + 1);
        }
        return best;
    }
}
""",
    "Two Pointer": r"""
class TwoPointerPairSum {
    static boolean hasPair(int[] sorted, int target) {
        int l = 0, r = sorted.length - 1;
        while (l < r) {
            int sum = sorted[l] + sorted[r];
            if (sum == target) return true;
            if (sum < target) l++;
            else r--;
        }
        return false;
    }
}
""",
    "Greedy": r"""
import java.util.*;

class IntervalScheduling {
    static int maxNonOverlapping(int[][] intervals) {
        Arrays.sort(intervals, Comparator.comparingInt(a -> a[1]));
        int count = 0, lastEnd = Integer.MIN_VALUE;
        for (int[] in : intervals) {
            if (in[0] >= lastEnd) {
                count++;
                lastEnd = in[1];
            }
        }
        return count;
    }
}
""",
    "Binary Trees": r"""
import java.util.*;

class TreeTraversal {
    static class TreeNode { int val; TreeNode left, right; TreeNode(int v) { val = v; } }
    static List<Integer> inorder(TreeNode root) {
        List<Integer> ans = new ArrayList<>();
        Deque<TreeNode> st = new ArrayDeque<>();
        TreeNode cur = root;
        while (cur != null || !st.isEmpty()) {
            while (cur != null) { st.push(cur); cur = cur.left; }
            cur = st.pop();
            ans.add(cur.val);
            cur = cur.right;
        }
        return ans;
    }
}
""",
    "BST": r"""
class ValidateBST {
    static class TreeNode { int val; TreeNode left, right; TreeNode(int v) { val = v; } }
    static boolean isValid(TreeNode root) {
        return dfs(root, Long.MIN_VALUE, Long.MAX_VALUE);
    }
    static boolean dfs(TreeNode node, long low, long high) {
        if (node == null) return true;
        if (node.val <= low || node.val >= high) return false;
        return dfs(node.left, low, node.val) && dfs(node.right, node.val, high);
    }
}
""",
    "Heap": r"""
import java.util.*;

class TopKFrequent {
    static List<Integer> topK(int[] nums, int k) {
        Map<Integer, Integer> freq = new HashMap<>();
        for (int x : nums) freq.put(x, freq.getOrDefault(x, 0) + 1);
        PriorityQueue<Integer> pq = new PriorityQueue<>(Comparator.comparingInt(freq::get));
        for (int x : freq.keySet()) {
            pq.offer(x);
            if (pq.size() > k) pq.poll();
        }
        return new ArrayList<>(pq);
    }
}
""",
    "Trie": r"""
class Trie {
    static class Node {
        Node[] next = new Node[26];
        boolean end;
    }
    private final Node root = new Node();
    void insert(String word) {
        Node cur = root;
        for (char ch : word.toCharArray()) {
            int i = ch - 'a';
            if (cur.next[i] == null) cur.next[i] = new Node();
            cur = cur.next[i];
        }
        cur.end = true;
    }
    boolean startsWith(String prefix) {
        Node cur = root;
        for (char ch : prefix.toCharArray()) {
            cur = cur.next[ch - 'a'];
            if (cur == null) return false;
        }
        return true;
    }
}
""",
    "Graphs": r"""
import java.util.*;

class GraphTraversal {
    static List<Integer> bfs(List<List<Integer>> g, int start) {
        boolean[] seen = new boolean[g.size()];
        List<Integer> order = new ArrayList<>();
        Queue<Integer> q = new ArrayDeque<>();
        seen[start] = true;
        q.offer(start);
        while (!q.isEmpty()) {
            int u = q.poll();
            order.add(u);
            for (int v : g.get(u)) if (!seen[v]) {
                seen[v] = true;
                q.offer(v);
            }
        }
        return order;
    }
}
""",
    "Topological Sorting": r"""
import java.util.*;

class KahnTopo {
    static List<Integer> topo(int n, int[][] edges) {
        List<List<Integer>> g = new ArrayList<>();
        for (int i = 0; i < n; i++) g.add(new ArrayList<>());
        int[] indeg = new int[n];
        for (int[] e : edges) { g.get(e[0]).add(e[1]); indeg[e[1]]++; }
        Queue<Integer> q = new ArrayDeque<>();
        for (int i = 0; i < n; i++) if (indeg[i] == 0) q.offer(i);
        List<Integer> ans = new ArrayList<>();
        while (!q.isEmpty()) {
            int u = q.poll();
            ans.add(u);
            for (int v : g.get(u)) if (--indeg[v] == 0) q.offer(v);
        }
        return ans.size() == n ? ans : List.of(); // empty means cycle
    }
}
""",
    "Shortest Path": r"""
import java.util.*;

class Dijkstra {
    static long[] shortest(int n, List<int[]>[] g, int src) {
        long[] dist = new long[n];
        Arrays.fill(dist, Long.MAX_VALUE / 4);
        dist[src] = 0;
        PriorityQueue<long[]> pq = new PriorityQueue<>(Comparator.comparingLong(a -> a[1]));
        pq.offer(new long[]{src, 0});
        while (!pq.isEmpty()) {
            long[] cur = pq.poll();
            int u = (int) cur[0];
            if (cur[1] != dist[u]) continue;
            for (int[] e : g[u]) {
                int v = e[0], w = e[1];
                if (dist[u] + w < dist[v]) {
                    dist[v] = dist[u] + w;
                    pq.offer(new long[]{v, dist[v]});
                }
            }
        }
        return dist;
    }
}
""",
    "MST": r"""
import java.util.*;

class KruskalMST {
    static class DSU {
        int[] p, sz;
        DSU(int n) { p = new int[n]; sz = new int[n]; for (int i = 0; i < n; i++) { p[i] = i; sz[i] = 1; } }
        int find(int x) { return p[x] == x ? x : (p[x] = find(p[x])); }
        boolean union(int a, int b) {
            a = find(a); b = find(b);
            if (a == b) return false;
            if (sz[a] < sz[b]) { int t = a; a = b; b = t; }
            p[b] = a; sz[a] += sz[b]; return true;
        }
    }
    static int mstCost(int n, int[][] edges) {
        Arrays.sort(edges, Comparator.comparingInt(e -> e[2]));
        DSU dsu = new DSU(n);
        int cost = 0;
        for (int[] e : edges) if (dsu.union(e[0], e[1])) cost += e[2];
        return cost;
    }
}
""",
    "Disjoint Set Union": r"""
class UnionFind {
    int[] parent, size;
    UnionFind(int n) {
        parent = new int[n];
        size = new int[n];
        for (int i = 0; i < n; i++) { parent[i] = i; size[i] = 1; }
    }
    int find(int x) {
        if (parent[x] != x) parent[x] = find(parent[x]);
        return parent[x];
    }
    boolean union(int a, int b) {
        a = find(a); b = find(b);
        if (a == b) return false;
        if (size[a] < size[b]) { int t = a; a = b; b = t; }
        parent[b] = a; size[a] += size[b];
        return true;
    }
}
""",
    "Advanced Graph Algorithms": r"""
import java.util.*;

class BipartiteCheck {
    static boolean isBipartite(List<List<Integer>> g) {
        int[] color = new int[g.size()];
        Arrays.fill(color, -1);
        for (int s = 0; s < g.size(); s++) if (color[s] == -1) {
            Queue<Integer> q = new ArrayDeque<>();
            color[s] = 0; q.offer(s);
            while (!q.isEmpty()) {
                int u = q.poll();
                for (int v : g.get(u)) {
                    if (color[v] == -1) { color[v] = color[u] ^ 1; q.offer(v); }
                    else if (color[v] == color[u]) return false;
                }
            }
        }
        return true;
    }
}
""",
    "Tarjan": r"""
import java.util.*;

class Bridges {
    int timer = 0;
    List<int[]> bridges = new ArrayList<>();
    void dfs(int u, int parent, List<List<Integer>> g, int[] tin, int[] low) {
        tin[u] = low[u] = ++timer;
        for (int v : g.get(u)) {
            if (v == parent) continue;
            if (tin[v] == 0) {
                dfs(v, u, g, tin, low);
                low[u] = Math.min(low[u], low[v]);
                if (low[v] > tin[u]) bridges.add(new int[]{u, v});
            } else {
                low[u] = Math.min(low[u], tin[v]);
            }
        }
    }
}
""",
    "Kosaraju": r"""
import java.util.*;

class KosarajuSCC {
    static void dfs(int u, List<List<Integer>> g, boolean[] seen, List<Integer> out) {
        seen[u] = true;
        for (int v : g.get(u)) if (!seen[v]) dfs(v, g, seen, out);
        out.add(u);
    }
    static int countScc(List<List<Integer>> g, List<List<Integer>> rg) {
        int n = g.size();
        boolean[] seen = new boolean[n];
        List<Integer> order = new ArrayList<>();
        for (int i = 0; i < n; i++) if (!seen[i]) dfs(i, g, seen, order);
        Arrays.fill(seen, false);
        int scc = 0;
        Collections.reverse(order);
        for (int u : order) if (!seen[u]) {
            dfs(u, rg, seen, new ArrayList<>());
            scc++;
        }
        return scc;
    }
}
""",
    "Dynamic Programming": r"""
class Knapsack01 {
    static int maxValue(int[] wt, int[] val, int cap) {
        int[] dp = new int[cap + 1];
        for (int i = 0; i < wt.length; i++) {
            for (int c = cap; c >= wt[i]; c--) {
                dp[c] = Math.max(dp[c], val[i] + dp[c - wt[i]]);
            }
        }
        return dp[cap];
    }
}
""",
    "Bit Manipulation": r"""
class BitTricks {
    static boolean isSet(int x, int bit) { return (x & (1 << bit)) != 0; }
    static int setBit(int x, int bit) { return x | (1 << bit); }
    static int clearBit(int x, int bit) { return x & ~(1 << bit); }
    static int countSetBits(int x) {
        int count = 0;
        while (x != 0) {
            x &= x - 1;
            count++;
        }
        return count;
    }
}
""",
    "Backtracking": r"""
import java.util.*;

class Permutations {
    static void permute(int[] a, int idx, List<List<Integer>> ans) {
        if (idx == a.length) {
            List<Integer> one = new ArrayList<>();
            for (int x : a) one.add(x);
            ans.add(one);
            return;
        }
        for (int i = idx; i < a.length; i++) {
            swap(a, idx, i);
            permute(a, idx + 1, ans);
            swap(a, idx, i);
        }
    }
    static void swap(int[] a, int i, int j) { int t = a[i]; a[i] = a[j]; a[j] = t; }
}
""",
    "Segment Tree": r"""
class SegmentTree {
    int n;
    long[] tree;
    SegmentTree(int[] a) { n = a.length; tree = new long[4 * n]; build(a, 1, 0, n - 1); }
    void build(int[] a, int node, int l, int r) {
        if (l == r) { tree[node] = a[l]; return; }
        int m = (l + r) >>> 1;
        build(a, node * 2, l, m);
        build(a, node * 2 + 1, m + 1, r);
        tree[node] = tree[node * 2] + tree[node * 2 + 1];
    }
    long query(int node, int l, int r, int ql, int qr) {
        if (qr < l || r < ql) return 0;
        if (ql <= l && r <= qr) return tree[node];
        int m = (l + r) >>> 1;
        return query(node * 2, l, m, ql, qr) + query(node * 2 + 1, m + 1, r, ql, qr);
    }
}
""",
    "Fenwick Tree": r"""
class Fenwick {
    long[] bit;
    Fenwick(int n) { bit = new long[n + 1]; }
    void add(int idx, long delta) {
        for (idx++; idx < bit.length; idx += idx & -idx) bit[idx] += delta;
    }
    long sumPrefix(int idx) {
        long ans = 0;
        for (idx++; idx > 0; idx -= idx & -idx) ans += bit[idx];
        return ans;
    }
    long rangeSum(int l, int r) { return sumPrefix(r) - (l == 0 ? 0 : sumPrefix(l - 1)); }
}
""",
    "Binary Lifting": r"""
import java.util.*;

class KthAncestor {
    int LOG;
    int[][] up;
    KthAncestor(int[] parent) {
        int n = parent.length;
        LOG = 1;
        while ((1 << LOG) <= n) LOG++;
        up = new int[LOG][n];
        up[0] = parent.clone();
        for (int j = 1; j < LOG; j++)
            for (int v = 0; v < n; v++)
                up[j][v] = up[j - 1][v] == -1 ? -1 : up[j - 1][up[j - 1][v]];
    }
    int kth(int node, int k) {
        for (int j = 0; j < LOG && node != -1; j++) if (((k >> j) & 1) == 1) node = up[j][node];
        return node;
    }
}
""",
    "Euler Tour": r"""
import java.util.*;

class EulerTour {
    int timer = 0;
    int[] tin, tout;
    void dfs(int u, int parent, List<List<Integer>> tree) {
        tin[u] = timer++;
        for (int v : tree.get(u)) if (v != parent) dfs(v, u, tree);
        tout[u] = timer - 1;
    }
    boolean isAncestor(int u, int v) {
        return tin[u] <= tin[v] && tout[v] <= tout[u];
    }
}
""",
    "Sparse Table": r"""
class SparseTableMin {
    int[][] st;
    int[] log;
    SparseTableMin(int[] a) {
        int n = a.length;
        log = new int[n + 1];
        for (int i = 2; i <= n; i++) log[i] = log[i / 2] + 1;
        st = new int[log[n] + 1][n];
        st[0] = a.clone();
        for (int j = 1; j < st.length; j++)
            for (int i = 0; i + (1 << j) <= n; i++)
                st[j][i] = Math.min(st[j - 1][i], st[j - 1][i + (1 << (j - 1))]);
    }
    int query(int l, int r) {
        int j = log[r - l + 1];
        return Math.min(st[j][l], st[j][r - (1 << j) + 1]);
    }
}
""",
    "LCA": r"""
class LCA {
    int LOG, timer = 0;
    int[][] up;
    int[] tin, tout, depth;
    boolean isAncestor(int u, int v) { return tin[u] <= tin[v] && tout[v] <= tout[u]; }
    int lca(int u, int v) {
        if (isAncestor(u, v)) return u;
        if (isAncestor(v, u)) return v;
        for (int j = LOG - 1; j >= 0; j--) {
            int jump = up[j][u];
            if (jump != -1 && !isAncestor(jump, v)) u = jump;
        }
        return up[0][u];
    }
}
""",
    "Meet-in-the-Middle": r"""
import java.util.*;

class MeetInMiddle {
    static long countSubsetsAtMost(int[] a, int limit) {
        int mid = a.length / 2;
        List<Integer> left = sums(a, 0, mid), right = sums(a, mid, a.length);
        Collections.sort(right);
        long ans = 0;
        for (int x : left) {
            int idx = upperBound(right, limit - x);
            ans += idx;
        }
        return ans;
    }
    static List<Integer> sums(int[] a, int l, int r) {
        List<Integer> res = new ArrayList<>();
        for (int mask = 0; mask < (1 << (r - l)); mask++) {
            int s = 0;
            for (int i = l; i < r; i++) if (((mask >> (i - l)) & 1) == 1) s += a[i];
            res.add(s);
        }
        return res;
    }
    static int upperBound(List<Integer> a, int x) {
        int l = 0, r = a.size();
        while (l < r) { int m = (l + r) >>> 1; if (a.get(m) <= x) l = m + 1; else r = m; }
        return l;
    }
}
""",
}


TOPICS = [
    {"name": "Programming Basics", "part": "Foundations", "why": "Programming basics exist so the interview is about problem solving, not syntax panic. Clean input, loops, conditionals, arrays, and functions let you express an algorithm precisely.", "analogy": "Learning grammar before writing essays: you need verbs and sentence structure before you can write an argument.", "diagram": "input -> parse -> transform -> output", "ops": ["Read input safely and convert it to typed values.", "Decompose work into functions with one responsibility.", "Use arrays, loops, and conditionals to express deterministic control flow.", "Print results exactly as required by the judge."], "lc": ["Fizz Buzz", "Running Sum of 1d Array", "Valid Palindrome"]},
    {"name": "Complexity Analysis", "part": "Foundations", "why": "Complexity analysis was invented to compare algorithms without depending on a specific laptop, compiler, or input file. It predicts growth.", "analogy": "A city traffic map: one road is fine at midnight, but you need to know what happens at rush hour.", "diagram": "O(1) < O(log n) < O(n) < O(n log n) < O(n^2) < O(2^n)", "ops": ["Count dominant loops, recursive branches, and state transitions.", "Ignore constants only after identifying the real bottleneck.", "Separate input size variables such as n, m, V, and E.", "Explain space used by auxiliary structures and recursion stack."], "lc": ["Binary Search", "Merge Sorted Array", "Climbing Stairs"]},
    {"name": "Mathematics", "part": "Foundations", "why": "Mathematical tools reduce brute force. GCD, primes, modular arithmetic, combinatorics, and divisibility convert a search into direct reasoning.", "analogy": "A calculator's memory keys: small trusted formulas save repeated work.", "diagram": "a = b*q + r\nGCD(a,b) = GCD(b,r)", "ops": ["Compute GCD/LCM safely with overflow awareness.", "Use modular arithmetic for large answers.", "Precompute primes or factorials when many queries repeat.", "Convert counting problems into formulas before coding."], "lc": ["Count Primes", "Pow(x, n)", "Happy Number"]},
    {"name": "Recursion", "part": "Foundations", "why": "Recursion exists for problems whose solution contains smaller copies of itself. It makes trees, search, divide-and-conquer, and DP natural.", "analogy": "Opening nested folders until you reach files, then returning with the answer.", "diagram": "solve(n)\n |-- solve(n-1)\n |-- combine", "ops": ["Define a base case that stops the call chain.", "Define the recursive transition and what each call returns.", "Use backtracking undo steps when mutating shared state.", "Track stack depth to avoid accidental overflow."], "lc": ["Reverse Linked List", "Subsets", "Generate Parentheses"]},
    {"name": "Hashing", "part": "Foundations", "why": "Hashing trades extra memory for fast lookup. It exists because repeated linear searches become too slow.", "analogy": "A library index card: the title points directly to the shelf instead of scanning every book.", "diagram": "key -> hash(key) -> bucket -> value", "ops": ["Insert keys with associated counts or values.", "Search for complements and previously seen states.", "Delete or decrement counts when a sliding window moves.", "Choose ordered maps when sorted traversal matters."], "lc": ["Two Sum", "Valid Anagram", "Longest Consecutive Sequence"]},
    {"name": "Sorting", "part": "Core Techniques", "why": "Sorting reveals structure. Many hard problems become simple when equal values group together or order enables two pointers and binary search.", "analogy": "Arranging books alphabetically before finding duplicates.", "diagram": "[5,1,4,2] -> [1,2,4,5]", "ops": ["Compare and swap or merge elements into order.", "Use stable sorting when original order matters.", "Sort custom objects with Comparator.", "Use sorting as a preprocessing step for greedy decisions."], "lc": ["Sort Colors", "Merge Intervals", "Count Inversions"]},
    {"name": "Arrays", "part": "Core Techniques", "why": "Arrays are the default container for contiguous data. They exist because indexed memory gives constant-time access and compact storage.", "analogy": "Numbered lockers in a hallway: locker 17 is reachable directly.", "diagram": "index: 0  1  2  3\nvalue: 8  4  9  2", "ops": ["Access by index.", "Traverse left-to-right or right-to-left.", "Update values in place.", "Shift values for insertion/deletion when needed."], "lc": ["Move Zeroes", "Best Time to Buy and Sell Stock", "Trapping Rain Water"]},
    {"name": "Prefix Sum", "part": "Core Techniques", "why": "Prefix sums exist to answer repeated range-sum questions in constant time after one linear pass.", "analogy": "A running bill total: to know the cost from item 4 to item 9, subtract the total before item 4.", "diagram": "a:    2  1  3  4\npref: 0  2  3  6 10\nsum[1..3] = pref[4]-pref[1]", "ops": ["Build cumulative totals.", "Answer range queries with subtraction.", "Combine with hashing for subarray-sum counts.", "Use 2D prefix sums for grids."], "lc": ["Range Sum Query - Immutable", "Subarray Sum Equals K", "Maximum Size Subarray Sum Equals k"]},
    {"name": "Difference Array", "part": "Core Techniques", "why": "Difference arrays exist because many range updates can be represented by only two boundary changes.", "analogy": "A thermostat schedule: mark when heat turns up and when it turns down, then replay the timeline.", "diagram": "add +5 on [1,3]\ndiff[1]+=5, diff[4]-=5\nprefix(diff) gives final values", "ops": ["Mark range starts and ends.", "Reconstruct final values by prefix accumulation.", "Use for interval increments and sweep-line counts.", "Extend to 2D with rectangle corner updates."], "lc": ["Car Pooling", "Corporate Flight Bookings", "Range Addition"]},
    {"name": "Kadane", "part": "Core Techniques", "why": "Kadane's algorithm exists to find the best contiguous subarray without testing all O(n^2) ranges.", "analogy": "Running with a backpack: if the current load becomes harmful, drop it and start fresh.", "diagram": "cur = max(a[i], cur + a[i])\nbest = max(best, cur)", "ops": ["Maintain best subarray ending at current index.", "Reset when previous sum hurts future choices.", "Track global best.", "Adapt to circular arrays and max product variants."], "lc": ["Maximum Subarray", "Maximum Sum Circular Subarray", "Maximum Product Subarray"]},
    {"name": "Binary Search", "part": "Core Techniques", "why": "Binary search exists to exploit monotonic structure. Each comparison discards half the candidates.", "analogy": "Guessing a number by asking higher or lower.", "diagram": "false false false true true true\n                 ^ first true", "ops": ["Search exact values in sorted arrays.", "Find lower/upper bounds.", "Search the answer space using a feasibility check.", "Avoid overflow with lo + (hi - lo) / 2."], "lc": ["Binary Search", "Search in Rotated Sorted Array", "Median of Two Sorted Arrays"]},
    {"name": "Strings", "part": "Core Techniques", "why": "String algorithms exist because text is sequential but comparisons can be expensive. Good preprocessing avoids repeated matching.", "analogy": "Autocomplete on a phone: the system uses structure instead of comparing every word from scratch.", "diagram": "pattern: a b a b a\npi:      0 0 1 2 3", "ops": ["Scan characters and maintain state.", "Normalize case or frequency when order does not matter.", "Use KMP/Z/Rabin-Karp for pattern matching.", "Use StringBuilder for repeated construction."], "lc": ["Valid Palindrome", "Longest Palindromic Substring", "Minimum Window Substring"]},
    {"name": "Linked List", "part": "Data Structures", "why": "Linked lists exist for dynamic insertion and deletion without shifting large contiguous arrays.", "analogy": "A treasure hunt: each clue points to the next location.", "diagram": "10 -> 20 -> 30 -> NULL", "ops": ["Traverse by following next pointers.", "Insert by rewiring links.", "Delete by bypassing a node.", "Reverse using prev/current/next pointers."], "lc": ["Middle of the Linked List", "Reverse Linked List", "Merge k Sorted Lists"]},
    {"name": "Stack", "part": "Data Structures", "why": "Stacks model nested or most-recent-first work. They exist because recursion, parentheses, and undo systems need LIFO behavior.", "analogy": "A stack of plates: the last plate placed is the first removed.", "diagram": "top\n30\n20\n10", "ops": ["Push a new element.", "Pop the most recent element.", "Peek without removing.", "Use monotonic variants to answer nearest-greater/smaller questions."], "lc": ["Valid Parentheses", "Min Stack", "Largest Rectangle in Histogram"]},
    {"name": "Queue", "part": "Data Structures", "why": "Queues model first-come-first-served processing. They exist for scheduling, BFS, and streaming buffers.", "analogy": "People standing in a ticket line.", "diagram": "Front -> 10 20 30 <- Rear", "ops": ["Enqueue at the rear.", "Dequeue from the front.", "Peek the front element.", "Use queue layers for BFS shortest steps in unweighted graphs."], "lc": ["Implement Queue using Stacks", "Rotting Oranges", "Number of Islands"]},
    {"name": "Monotonic Stack", "part": "Data Structures", "why": "A monotonic stack exists to remember only candidates that can still become the answer.", "analogy": "A line of taller buildings blocking shorter buildings behind them.", "diagram": "incoming x pops smaller elements\nstack remains decreasing", "ops": ["Push indexes, not just values, when distances are needed.", "Pop while the invariant is violated.", "Answer for popped elements immediately.", "Handle equal values deliberately."], "lc": ["Next Greater Element I", "Daily Temperatures", "Largest Rectangle in Histogram"]},
    {"name": "Monotonic Queue", "part": "Data Structures", "why": "A monotonic queue exists to answer sliding-window min/max queries in linear time.", "analogy": "A shortlist of the strongest candidates; anyone worse and older gets removed.", "diagram": "window [l..r]\nfront = best value\nback = newest candidate", "ops": ["Remove expired indexes from the front.", "Pop worse candidates from the back.", "Read best value at the front.", "Push current index after cleanup."], "lc": ["Sliding Window Maximum", "Shortest Subarray with Sum at Least K", "Constrained Subsequence Sum"]},
    {"name": "Sliding Window", "part": "Patterns", "why": "Sliding window exists for contiguous subarray/substring problems where expanding and shrinking a range preserves enough information.", "analogy": "Moving a camera frame across a long table.", "diagram": "left        right\n  |-----------|", "ops": ["Expand right to include new data.", "Shrink left while the window violates a condition.", "Maintain counts, sum, or distinct elements.", "Convert exactly-K counts with atMost(K)-atMost(K-1)."], "lc": ["Maximum Average Subarray I", "Longest Substring Without Repeating Characters", "Minimum Window Substring"]},
    {"name": "Two Pointer", "part": "Patterns", "why": "Two pointers exist when one pass with coordinated indexes can replace nested loops.", "analogy": "Two people walking from opposite ends of a bookshelf toward the right pair.", "diagram": "l -> 1 3 4 8 10 <- r", "ops": ["Move the pointer whose side cannot produce a better answer.", "Use fast/slow pointers for linked-list cycles and middle nodes.", "Merge two sorted sequences.", "Skip duplicates carefully in combination problems."], "lc": ["Two Sum II", "Container With Most Water", "3Sum"]},
    {"name": "Greedy", "part": "Patterns", "why": "Greedy algorithms exist when a locally best choice can be proven not to harm the global optimum.", "analogy": "Choosing the earliest-finishing meeting so the room frees up soonest.", "diagram": "sort -> choose valid best -> repeat", "ops": ["Find the exchange argument or invariant.", "Sort by the criterion that makes the choice safe.", "Use heap when the current best changes dynamically.", "Reject greedy if a local choice can trap future options."], "lc": ["Assign Cookies", "Jump Game", "Non-overlapping Intervals"]},
    {"name": "Binary Trees", "part": "Trees", "why": "Trees model hierarchy. Binary trees appear in parsing, search structures, decision trees, and recursive decomposition.", "analogy": "An org chart where each manager has up to two direct branches.", "diagram": "    10\n   /  \\\n  5   15\n / \\\n2   7", "ops": ["Traverse preorder, inorder, postorder, and level order.", "Compute height, diameter, balance, and views.", "Build trees from traversal arrays.", "Use recursion to return facts from children to parent."], "lc": ["Maximum Depth of Binary Tree", "Binary Tree Level Order Traversal", "Serialize and Deserialize Binary Tree"]},
    {"name": "BST", "part": "Trees", "why": "A BST exists to keep values ordered so search, insertion, and range queries follow one branch instead of scanning everything.", "analogy": "A dictionary split by alphabet ranges.", "diagram": "left values < node < right values", "ops": ["Search by comparing with node value.", "Insert at the first null branch.", "Delete with leaf/one-child/two-child cases.", "Use inorder traversal for sorted output."], "lc": ["Search in a Binary Search Tree", "Validate Binary Search Tree", "Kth Smallest Element in a BST"]},
    {"name": "Heap", "part": "Data Structures", "why": "Heaps exist for repeated access to the smallest or largest current item.", "analogy": "A hospital emergency queue where highest priority is treated first.", "diagram": "      1\n    /   \\\n   3     5\n  / \\\n 8   9", "ops": ["Offer an element and bubble it upward.", "Poll root and heapify downward.", "Peek the priority element.", "Use two heaps for medians or scheduling."], "lc": ["Kth Largest Element in an Array", "Top K Frequent Elements", "Find Median from Data Stream"]},
    {"name": "Trie", "part": "Data Structures", "why": "Tries exist to share prefixes between words and answer prefix queries efficiently.", "analogy": "Mobile keyboard autocomplete.", "diagram": "root\n |-- c -- a -- t\n |        \\-- r", "ops": ["Insert characters node by node.", "Search full words with end markers.", "Search prefixes without requiring end markers.", "Use bitwise tries for maximum XOR."], "lc": ["Implement Trie", "Word Search II", "Maximum XOR of Two Numbers in an Array"]},
    {"name": "Graphs", "part": "Graphs", "why": "Graphs exist because many relationships are networks rather than lines or hierarchies.", "analogy": "Google Maps: cities are nodes, roads are edges.", "diagram": "0 -- 1\n|    |\n2 -- 3", "ops": ["Represent with adjacency list, matrix, or grid offsets.", "Traverse with BFS or DFS.", "Detect connected components, cycles, and bipartition.", "Respect directed vs undirected edge semantics."], "lc": ["Number of Islands", "Clone Graph", "Course Schedule"]},
    {"name": "Topological Sorting", "part": "Graphs", "why": "Topological sorting exists to order tasks that have prerequisites.", "analogy": "Taking courses only after completing prerequisites.", "diagram": "A -> C\nB -> C\norder: A, B, C", "ops": ["Compute indegrees.", "Start with zero-indegree nodes.", "Remove edges as tasks complete.", "If processed count is smaller than V, a cycle exists."], "lc": ["Course Schedule", "Course Schedule II", "Alien Dictionary"]},
    {"name": "Shortest Path", "part": "Graphs", "why": "Shortest path algorithms exist to minimize cost through a network under different edge-weight rules.", "analogy": "Navigation choosing the fastest route, not simply the fewest turns.", "diagram": "unweighted -> BFS\nnonnegative -> Dijkstra\nnegative edges -> Bellman-Ford", "ops": ["Choose algorithm by edge weights.", "Relax edges when a better distance appears.", "Use priority queues for Dijkstra.", "Detect negative cycles when required."], "lc": ["Network Delay Time", "Cheapest Flights Within K Stops", "Path With Minimum Effort"]},
    {"name": "MST", "part": "Graphs", "why": "Minimum spanning trees exist to connect all nodes with minimum total edge cost and no cycles.", "analogy": "Laying cable between offices as cheaply as possible while keeping everyone connected.", "diagram": "sort edges by weight -> add if it connects components", "ops": ["Sort edges for Kruskal.", "Use DSU to avoid cycles.", "Use Prim with a heap from any start.", "Verify graph connectivity if all nodes must be connected."], "lc": ["Min Cost to Connect All Points", "Connecting Cities With Minimum Cost", "Optimize Water Distribution in a Village"]},
    {"name": "Disjoint Set Union", "part": "Graphs", "why": "DSU exists to maintain dynamic connectivity between components with near-constant operations.", "analogy": "Merging friend groups and asking whether two people are already in the same group.", "diagram": "parent[x] -> representative", "ops": ["Find the representative of a node.", "Compress paths during find.", "Union smaller tree into larger tree.", "Use for cycles, components, and Kruskal MST."], "lc": ["Number of Connected Components in an Undirected Graph", "Redundant Connection", "Accounts Merge"]},
    {"name": "Advanced Graph Algorithms", "part": "Graphs", "why": "Advanced graph algorithms exist for structure beyond reachability: cuts, components, parity, flows, and dependency condensation.", "analogy": "Urban planning: not just whether roads exist, but which bridges are critical and which districts are inseparable.", "diagram": "graph -> structural property -> compressed answer", "ops": ["Classify graph type and constraints.", "Track discovery times or colors.", "Compress components when cycles obscure DAG structure.", "Use the weakest algorithm that proves the property needed."], "lc": ["Is Graph Bipartite?", "Critical Connections in a Network", "Reconstruct Itinerary"]},
    {"name": "Tarjan", "part": "Graphs", "why": "Tarjan-style low-link algorithms exist to discover bridges, articulation points, and strongly connected structure in one DFS.", "analogy": "Testing which road closures split a city into disconnected neighborhoods.", "diagram": "tin[u] = entry time\nlow[u] = earliest reachable ancestor", "ops": ["Assign entry times during DFS.", "Update low values from back edges.", "Identify bridge when low[child] > tin[parent].", "Handle parent edges carefully in undirected graphs."], "lc": ["Critical Connections in a Network", "Minimum Number of Days to Disconnect Island", "Find Critical and Pseudo-Critical Edges in MST"]},
    {"name": "Kosaraju", "part": "Graphs", "why": "Kosaraju exists to find strongly connected components in directed graphs using two DFS passes.", "analogy": "Groups of rooms where every room can reach every other room by one-way corridors.", "diagram": "DFS finish order -> reverse graph -> collect SCCs", "ops": ["Run DFS and store finish order.", "Reverse all directed edges.", "Process nodes in reverse finish order.", "Each DFS in reversed graph is one SCC."], "lc": ["Strongly Connected Components", "Minimum Vertices to Reach All Nodes", "Longest Cycle in a Graph"]},
    {"name": "Dynamic Programming", "part": "Dynamic Programming", "why": "DP exists when brute force repeats the same subproblems. It stores answers so each state is solved once.", "analogy": "A spreadsheet where each cell depends on earlier cells, and recalculation reuses saved values.", "diagram": "state -> transition -> base case -> answer", "ops": ["Define the state precisely.", "Write recurrence from smaller states.", "Choose memoization or tabulation.", "Optimize space only after correctness is clear."], "lc": ["Climbing Stairs", "Longest Increasing Subsequence", "Edit Distance"]},
    {"name": "Bit Manipulation", "part": "Advanced Techniques", "why": "Bit manipulation exists because integers are binary sets. It makes parity, masks, subsets, and XOR tricks compact and fast.", "analogy": "A row of switches where each switch represents a yes/no property.", "diagram": "mask = 10110\nbit 2 set? mask & (1<<2)", "ops": ["Set, clear, toggle, and test bits.", "Use XOR for cancellation and parity.", "Enumerate subsets with masks.", "Use bitmask DP when n is small but combinations matter."], "lc": ["Single Number", "Counting Bits", "Maximum XOR of Two Numbers in an Array"]},
    {"name": "Backtracking", "part": "Advanced Techniques", "why": "Backtracking exists for controlled exhaustive search where partial decisions can be undone.", "analogy": "Trying keys on a lock, removing the wrong key, and trying the next.", "diagram": "choose -> explore -> undo", "ops": ["Track the current path.", "Mark used choices.", "Prune invalid partial states early.", "Undo every mutation before returning."], "lc": ["Subsets", "Combination Sum", "N-Queens"]},
    {"name": "Segment Tree", "part": "Advanced Data Structures", "why": "Segment trees exist for fast range queries with updates when prefix sums are not enough.", "analogy": "A tournament bracket where each parent summarizes a range of players.", "diagram": "[0..7]\n /    \\\n[0..3] [4..7]", "ops": ["Build aggregate values bottom-up.", "Query by splitting overlapping ranges.", "Update a point or range.", "Use lazy propagation for range updates."], "lc": ["Range Sum Query - Mutable", "Count of Smaller Numbers After Self", "My Calendar III"]},
    {"name": "Fenwick Tree", "part": "Advanced Data Structures", "why": "Fenwick trees exist as a compact alternative for prefix queries and point updates.", "analogy": "A set of labeled buckets, each bucket stores a power-of-two block summary.", "diagram": "idx += idx & -idx  // update\nidx -= idx & -idx  // query", "ops": ["Add delta to an index.", "Compute prefix sum.", "Convert range sum by subtracting prefixes.", "Use coordinate compression for large values."], "lc": ["Range Sum Query - Mutable", "Count of Smaller Numbers After Self", "Reverse Pairs"]},
    {"name": "Binary Lifting", "part": "Advanced Trees", "why": "Binary lifting exists to jump through ancestors in powers of two instead of walking one edge at a time.", "analogy": "Taking elevator express stops: 1 floor, 2 floors, 4 floors, 8 floors.", "diagram": "up[j][v] = 2^j-th ancestor of v", "ops": ["Precompute jump table.", "Lift by bits of k.", "Equalize depths before LCA.", "Combine with min/max edge tables for path queries."], "lc": ["Kth Ancestor of a Tree Node", "Lowest Common Ancestor of a Binary Tree", "Tree Queries"]},
    {"name": "Euler Tour", "part": "Advanced Trees", "why": "Euler tours flatten trees so subtree questions become range questions.", "analogy": "Walking through a museum and timestamping when you enter and leave each room.", "diagram": "tin[u] <= tin[v] <= tout[u] means v is in u's subtree", "ops": ["DFS and record entry/exit times.", "Flatten subtree to contiguous range.", "Pair with Fenwick/segment tree for updates.", "Use tin/tout for ancestor checks."], "lc": ["Subtree Queries", "Employee Importance", "Time Needed to Inform All Employees"]},
    {"name": "Sparse Table", "part": "Advanced Data Structures", "why": "Sparse tables exist for immutable range queries where preprocessing can make each query extremely fast.", "analogy": "A travel guide that precomputes best hotels for every block length.", "diagram": "st[k][i] summarizes range [i, i+2^k-1]", "ops": ["Precompute powers of two.", "Merge two half-blocks.", "Answer idempotent queries with two overlapping blocks.", "Use log table to select block size."], "lc": ["Range Minimum Query", "Sliding Window Maximum", "Static Range Queries"]},
    {"name": "LCA", "part": "Advanced Trees", "why": "Lowest common ancestor exists to answer tree path questions by finding where two root paths meet.", "analogy": "Finding the nearest shared manager of two employees.", "diagram": "root\n |-- a -- x\n |-- b -- y\nLCA(x,y)=root", "ops": ["Preprocess parent/depth.", "Equalize depths.", "Lift both nodes until parents match.", "Use LCA to compute distances and path aggregates."], "lc": ["Lowest Common Ancestor of a Binary Tree", "Lowest Common Ancestor of a BST", "Minimum Edge Weight Equilibrium Queries in a Tree"]},
    {"name": "Meet-in-the-Middle", "part": "Advanced Techniques", "why": "Meet-in-the-middle exists when n is too large for 2^n but small enough for two 2^(n/2) halves.", "analogy": "Two search parties start from opposite sides of a mountain and compare notes in the middle.", "diagram": "left subsets + right subsets -> combine by sort/binary search", "ops": ["Split the input into two halves.", "Enumerate all answers in each half.", "Sort one side.", "Use binary search or two pointers to combine."], "lc": ["Partition Array Into Two Arrays to Minimize Sum Difference", "Closest Subsequence Sum", "Split Array With Same Average"]},
]


def code_for(name: str) -> str:
    if name in CODE:
        return CODE[name]
    for key, code in CODE.items():
        if key in name or name in key:
            return code
    return CODE["Programming Basics"]


def add_topic(doc: Document, idx: int, topic: dict) -> None:
    doc.add_heading(topic["name"], level=1)
    add_callout(doc, "Interview lens", f"{topic['name']} questions usually test whether you can identify the invariant before writing code. Start by stating what must remain true after every operation.", "tip")

    add_subhead(doc, "1. Concept")
    add_para(doc, f"{topic['name']} is a way to organize computation so a repeated decision becomes predictable. In interviews, the concept is less about memorizing a template and more about knowing what information must be preserved while the input changes.")

    add_subhead(doc, "2. Why It Exists")
    add_para(doc, topic["why"])

    add_subhead(doc, "3. Real-World Analogy")
    add_para(doc, topic["analogy"])

    add_subhead(doc, "4. Theory")
    add_para(doc, f"The theory behind {topic['name']} is built around an invariant: a statement that stays true as the algorithm progresses. When you can name the invariant, you can prove why the algorithm is correct and explain it under pressure.")
    add_para(doc, "A practical interview approach is to write the brute force first in words, identify the repeated work, and then choose the data structure or pattern that removes that repetition.")

    add_subhead(doc, "5. Visual Diagram")
    add_code(doc, topic["diagram"])

    add_subhead(doc, "6. Operations")
    add_bullets(doc, topic["ops"])

    add_subhead(doc, "7. Time Complexity")
    add_table(doc, ["Operation", "Time", "Space", "Interview note"], base_complexities(topic["name"]), [2150, 1600, 1500, 4110])

    add_subhead(doc, "8. Java Implementation")
    add_code(doc, code_for(topic["name"]))

    add_subhead(doc, "9. Dry Run")
    sample = [
        f"Choose a tiny input and write the state before the first operation.",
        f"Apply the {topic['name']} invariant after each step, not only at the end.",
        "Record the moment the answer changes; this exposes off-by-one mistakes early.",
        "Compare the final result with brute force for the same small input."
    ]
    add_numbers(doc, sample)

    add_subhead(doc, "10. Interview Tricks")
    add_callout(doc, "Common trap", f"Do not jump to the template for {topic['name']} before reading constraints. Constraints decide whether the intended solution is O(n), O(n log n), logarithmic, or exponential with pruning.", "warn")
    add_bullets(doc, [
        "Say the invariant out loud before coding.",
        "Use names that describe meaning, not just i, j, temp, and flag.",
        "Dry-run a boundary case: empty input, one element, duplicate values, or disconnected structure.",
    ])

    add_subhead(doc, "11. Pattern Recognition")
    add_bullets(doc, [
        f"Look for wording that implies {topic['name']}: repeated queries, ordered choices, connectivity, contiguous ranges, hierarchy, or state transitions.",
        "If a brute force solution recomputes the same fact, ask whether preprocessing, a map, a heap, DP, or a tree can store that fact.",
        "If the input is sorted or can be sorted without breaking meaning, consider binary search, two pointers, or greedy ordering.",
    ])

    add_subhead(doc, "12. Common Interview Questions")
    add_table(
        doc,
        ["Level", "Representative questions"],
        [
            ["Beginner", f"Explain {topic['name']} from first principles; implement the core operation; dry-run a small case."],
            ["Intermediate", f"Combine {topic['name']} with sorting, hashing, recursion, or another common pattern."],
            ["Advanced", f"Optimize memory, handle duplicates/edge cases, or prove the invariant under changing constraints."],
            ["FAANG", f"Recognize {topic['name']} inside a disguised story problem, discuss tradeoffs, and produce production-clean Java."],
        ],
        [1700, 7660],
        header_fill=LIGHT_TEAL,
    )

    add_subhead(doc, "13. Related LeetCode Problems")
    easy = topic.get("lc", ["Two Sum", "Valid Parentheses", "Binary Search"])
    medium = topic.get("medium", ["Combination Sum", "Course Schedule", "Longest Substring Without Repeating Characters"])
    hard = topic.get("hard", ["Median of Two Sorted Arrays", "Serialize and Deserialize Binary Tree", "Minimum Window Substring"])
    add_table(doc, ["Easy", "Medium", "Hard"], [[easy[0], medium[0], hard[0]], [easy[1], medium[1], hard[1]], [easy[2], medium[2], hard[2]]], [3120, 3120, 3120])

    add_subhead(doc, "14. Practice Roadmap")
    add_table(
        doc,
        ["Stage", "Focus", "Exit criteria"],
        [
            ["Easy", "Understand the operation and write the simplest correct version.", "You can solve without looking at notes."],
            ["Medium", "Add constraints, edge cases, and pattern combinations.", "You can explain why the complexity is enough."],
            ["Hard", "Optimize, prove, and handle adversarial cases.", "You can compare at least two approaches clearly."],
        ],
        [1500, 4300, 3560],
        header_fill=LIGHT_GOLD,
    )

    if idx != len(TOPICS):
        doc.add_page_break()


def add_foundation_overview(doc: Document) -> None:
    doc.add_heading("DSA Pattern Recognition", level=1)
    add_para(doc, "Pattern recognition is not guessing. It is a disciplined process of mapping problem wording, constraints, and required output to a small set of known transformations.")
    add_table(
        doc,
        ["Signal in problem", "Likely pattern", "First question to ask"],
        [
            ["Contiguous subarray or substring", "Sliding window, prefix sum, Kadane", "Does expanding right make the condition easier to maintain?"],
            ["Sorted array or monotonic answer", "Binary search", "Can I write a true/false check that flips once?"],
            ["Pairs/triples after sorting", "Two pointers", "Which pointer move discards impossible pairs?"],
            ["Network, grid, prerequisites", "Graph traversal/topological/shortest path", "Are edges weighted, directed, or cyclic?"],
            ["Repeated optimal subproblems", "Dynamic programming", "What is the smallest state that determines the future?"],
            ["Range query with updates", "Fenwick or segment tree", "Are updates point, range, or both?"],
        ],
        [2600, 2600, 4160],
    )
    add_callout(doc, "Tip", "When stuck, write the brute force recurrence or loops. The optimization usually comes from caching, sorting, indexing, or narrowing the search space.", "tip")
    doc.add_page_break()


def add_complexity_cheat(doc: Document) -> None:
    doc.add_heading("Complexity Analysis Cheat Sheet", level=1)
    add_table(
        doc,
        ["Notation", "Meaning", "Interview wording"],
        [
            ["Big-O", "Upper bound on growth", "The algorithm will not grow worse than this class."],
            ["Big-Theta", "Tight bound", "The algorithm grows exactly in this class asymptotically."],
            ["Big-Omega", "Lower bound", "The algorithm must take at least this much in the best guaranteed sense."],
            ["Amortized", "Average over a sequence", "Some operations are expensive, but the total sequence is controlled."],
            ["Recursive", "Cost defined by recurrence", "Count branches, work per level, and depth."],
            ["Master theorem", "Shortcut for T(n)=aT(n/b)+f(n)", "Compare f(n) with n^(log_b a)."],
        ],
        [1700, 3300, 4360],
    )
    add_bullets(doc, [
        "O(1): direct access or constant fixed work.",
        "O(log n): the search space shrinks by a factor each step.",
        "O(n): every element is processed a constant number of times.",
        "O(n log n): sorting or balanced divide-and-conquer.",
        "O(n^2): pair comparisons; usually too slow beyond about 10^5.",
        "O(2^n) and O(n!): subset/permutation search; require small n or pruning.",
    ])
    add_callout(doc, "Warning", "Never say a recursive algorithm is O(n) just because it has one parameter. Draw the recursion tree or define states.", "risk")
    doc.add_page_break()


def add_java_collections(doc: Document) -> None:
    doc.add_heading("Java Collections Cheat Sheet", level=1)
    add_table(
        doc,
        ["Collection", "Use when", "Core costs", "Interview warning"],
        [
            ["ArrayList", "Indexed dynamic array", "get O(1), add end amortized O(1)", "Middle insert/delete shifts elements."],
            ["LinkedList", "Deque-style operations", "add/remove ends O(1)", "Index access is O(n); rarely best for interviews."],
            ["ArrayDeque", "Stack or queue", "push/pop/offer/poll O(1)", "Prefer over legacy Stack."],
            ["HashMap", "Key-value lookup", "average O(1)", "Use getOrDefault and handle missing keys."],
            ["TreeMap", "Sorted keys/range queries", "O(log n)", "Use floorKey/ceilingKey for neighbor queries."],
            ["HashSet", "Membership", "average O(1)", "Custom objects need equals/hashCode."],
            ["TreeSet", "Ordered set", "O(log n)", "Comparator must be consistent."],
            ["PriorityQueue", "Heap", "offer/poll O(log n)", "Java PQ is min-heap by default."],
            ["Comparable", "Natural ordering", "sort/PQ support", "Return Integer.compare(a,b), not a-b."],
            ["Comparator", "Custom ordering", "sort/PQ support", "Avoid overflow in subtraction comparators."],
        ],
        [1800, 2800, 2300, 2460],
    )
    add_code(doc, r"""
Map<String, Integer> freq = new HashMap<>();
freq.put(word, freq.getOrDefault(word, 0) + 1);

PriorityQueue<int[]> pq = new PriorityQueue<>(Comparator.comparingInt(a -> a[1]));

Deque<Integer> stack = new ArrayDeque<>();
stack.push(10);
int top = stack.pop();

Arrays.sort(arr);
Collections.sort(list, Comparator.reverseOrder());
""")
    doc.add_page_break()


def add_pattern_handbooks(doc: Document) -> None:
    chapters = [
        ("Binary Search Pattern Handbook", [
            ["Exact search", "Find target in sorted array", "Maintain inclusive or half-open bounds consistently."],
            ["Lower/upper bound", "First index satisfying condition", "Use lo < hi and return lo."],
            ["Search on answer", "Minimum feasible capacity/time/radius", "Write a monotonic check first."],
            ["Rotated arrays", "One half is sorted", "Decide which half can contain target."],
            ["2D matrix", "Flattened sorted or staircase property", "Respect row/column monotonicity."],
        ]),
        ("Sliding Window Pattern Handbook", [
            ["Fixed window", "Exactly k length", "Initialize first k, then move one step at a time."],
            ["Variable window", "At most condition", "Expand right, shrink left while invalid."],
            ["At most K", "Count subarrays/substrings", "Every valid window contributes right-left+1."],
            ["Exactly K", "Exact count", "Compute atMost(K)-atMost(K-1)."],
            ["Min window", "Cover required characters", "Shrink after all requirements are satisfied."],
        ]),
        ("Graph Pattern Handbook", [
            ["DFS", "Components, recursion, cycle state", "Track parent for undirected graphs."],
            ["BFS", "Shortest unweighted paths", "Process by layers."],
            ["Grid", "Cells as nodes", "Use direction arrays and bounds checks."],
            ["Topological", "Prerequisites", "DAG only; cycle means impossible."],
            ["Shortest path", "Weighted navigation", "Choose BFS, Dijkstra, or Bellman-Ford by weights."],
            ["MST", "Connect all with minimum cost", "Kruskal uses DSU; Prim uses heap."],
            ["DSU", "Dynamic connectivity", "Path compression plus union by size."],
            ["Bipartite", "Two-color graph", "Odd cycle breaks it."],
        ]),
        ("DP Pattern Handbook", [
            ["0/1 Knapsack", "Take or skip each item once", "Iterate capacity backward for 1D optimization."],
            ["Unbounded", "Reuse items", "Iterate capacity forward."],
            ["LIS", "Increasing sequence", "O(n log n) tails array or O(n^2) DP."],
            ["LCS", "Two strings", "State i,j over prefixes."],
            ["Grid", "Move right/down or four directions", "Base row/column carefully."],
            ["Partition", "Split array/string", "Try last cut or subset sum."],
            ["Digit DP", "Count numbers with constraints", "State pos,tight,started,..."],
            ["Bitmask DP", "Small n assignment/TSP", "State mask and last/position."],
            ["Tree DP", "Parent-child decisions", "Return include/exclude or multiple states."],
            ["Interval DP", "Merge/partition intervals", "Process by length."],
            ["State compression", "Reduce memory", "Only keep states needed for next transition."],
        ]),
        ("Backtracking Pattern Handbook", [
            ["Subsets", "Choose/not choose", "No need to use every element."],
            ["Permutations", "Order matters", "Track used elements or swap in place."],
            ["Combination Sum", "Reuse or no reuse", "Control start index."],
            ["N Queens", "Constraint placement", "Track columns and diagonals."],
            ["Sudoku", "Fill constrained cells", "Choose next empty cell with fewest candidates."],
            ["Word Search", "Path in grid", "Mark visited, explore, then unmark."],
        ]),
    ]
    for title, rows in chapters:
        doc.add_heading(title, level=1)
        add_para(doc, f"{title} is a decision guide. Use it before coding to choose the smallest sufficient template and avoid over-engineering.")
        add_table(doc, ["Pattern", "Recognize it by", "Implementation focus"], rows, [2200, 3600, 3560], header_fill=LIGHT_TEAL)
        add_callout(doc, "Interview note", "A pattern name is not a proof. Always pair it with an invariant and a dry run.", "tip")
        doc.add_page_break()


def add_strategy_sections(doc: Document) -> None:
    doc.add_heading("FAANG Interview Strategy", level=1)
    add_table(
        doc,
        ["Company style", "What usually matters", "Preparation emphasis"],
        [
            ["Google", "Problem solving clarity, edge cases, generalization", "Practice explaining invariants before code."],
            ["Meta", "Speed, clean implementation, follow-up optimization", "Drill common patterns until templates are automatic."],
            ["Microsoft", "Balanced coding, communication, practical debugging", "Talk through tradeoffs and test cases steadily."],
            ["Amazon", "Correctness, ownership stories, scalable reasoning", "Pair DSA practice with leadership-principle examples."],
            ["Apple/Nvidia/Adobe/Atlassian/Uber", "Role-dependent depth and robust engineering taste", "Expect DSA plus system/product constraints."],
        ],
        [1900, 3650, 3810],
    )
    add_bullets(doc, [
        "Start every answer with brute force, then optimize.",
        "Write tests before declaring done: empty, one element, duplicates, extremes, and impossible cases.",
        "Narrate tradeoffs without rambling: time, space, correctness, and implementation risk.",
    ])
    doc.add_page_break()

    doc.add_heading("OA Strategy", level=1)
    add_table(
        doc,
        ["Phase", "Time box", "Action"],
        [
            ["Scan", "3-5 minutes", "Read all problems and identify obvious patterns."],
            ["First solve", "20-25 minutes", "Do the highest-confidence problem first."],
            ["Core attempt", "35-45 minutes", "Implement the main problem with careful tests."],
            ["Fallback", "Last 10 minutes", "Submit partial brute force only if constraints allow partial scoring."],
        ],
        [1700, 1700, 5960],
        header_fill=LIGHT_GOLD,
    )
    add_callout(doc, "Debugging strategy", "Print only small state locally. On platform submissions, reason from failed edge cases instead of flooding output.", "warn")
    doc.add_page_break()


def add_revision_plans(doc: Document) -> None:
    doc.add_heading("Revision Checklist", level=1)
    add_table(
        doc,
        ["Topic group", "Must be able to do"],
        [
            ["Foundations", "Explain Big-O, recursion base cases, hashing collisions, and Java collection costs."],
            ["Arrays and strings", "Use prefix sums, two pointers, sliding windows, Kadane, and binary search."],
            ["Linked structures", "Reverse lists, detect cycles, use stack/queue/deque, and handle null boundaries."],
            ["Trees", "Traverse, compute height/diameter/views, validate BST, and answer LCA-style questions."],
            ["Graphs", "Run BFS/DFS, topological sort, Dijkstra, MST, DSU, SCC, bridges, and grid traversal."],
            ["DP", "Define states for knapsack, LIS, LCS, grid, partition, interval, digit, tree, and bitmask DP."],
            ["Advanced DS", "Implement Fenwick, segment tree, sparse table, binary lifting, Euler tour, and trie."],
        ],
        [2400, 6960],
    )
    doc.add_page_break()

    plans = [
        ("30-Day Revision Plan", [
            ["Days 1-5", "Foundations, arrays, strings, hashing, sorting"],
            ["Days 6-10", "Binary search, two pointers, sliding window, prefix/difference/Kadane"],
            ["Days 11-15", "Linked list, stack, queue, heap, trie"],
            ["Days 16-21", "Trees, BST, recursion, backtracking"],
            ["Days 22-27", "Graphs, DSU, shortest path, MST, topo, SCC"],
            ["Days 28-30", "DP patterns, mock interviews, final weak spots"],
        ]),
        ("60-Day Preparation Plan", [
            ["Weeks 1-2", "Complete foundations and basic arrays/strings with daily dry runs."],
            ["Weeks 3-4", "Master binary search, sliding window, two pointers, linked list, stack, queue."],
            ["Weeks 5-6", "Trees, BST, heap, trie, backtracking, and greedy."],
            ["Weeks 7-8", "Graphs and DP with timed practice."],
            ["Final days", "Mock interviews and revision checklist."],
        ]),
        ("90-Day FAANG Preparation Roadmap", [
            ["Month 1", "Build foundations and solve breadth-first across all easy/medium topics."],
            ["Month 2", "Deepen graphs, DP, binary search on answer, and advanced data structures."],
            ["Month 3", "Timed mocks, hard problems, company-specific practice, and explanation polish."],
        ]),
        ("Final 7-Day Interview Revision", [
            ["Day 1", "Complexity, Java collections, arrays, strings."],
            ["Day 2", "Binary search, sliding window, two pointers."],
            ["Day 3", "Linked list, stack, queue, heap, trie."],
            ["Day 4", "Trees, BST, LCA, binary lifting."],
            ["Day 5", "Graphs, shortest path, MST, DSU, SCC."],
            ["Day 6", "DP and backtracking."],
            ["Day 7", "Mock interview, templates, rest, and logistics."],
        ]),
    ]
    for title, rows in plans:
        doc.add_heading(title, level=1)
        add_table(doc, ["Time", "Focus"], rows, [1900, 7460], header_fill=LIGHT_BLUE)
        add_callout(doc, "Rule", "Every day should include at least one dry run by hand and one reflection note on a mistake.", "tip")
        doc.add_page_break()


def add_appendix(doc: Document) -> None:
    doc.add_heading("Appendix: Big-O Cheat Sheet", level=1)
    add_complexity_cheat(doc)

    appendix_tables = [
        ("Sorting Comparison Table", ["Algorithm", "Best", "Average", "Worst", "Stable", "Use"], [
            ["Bubble", "O(n)", "O(n^2)", "O(n^2)", "Yes", "Learning only"],
            ["Insertion", "O(n)", "O(n^2)", "O(n^2)", "Yes", "Nearly sorted small arrays"],
            ["Merge", "O(n log n)", "O(n log n)", "O(n log n)", "Yes", "Stable sorting and inversion count"],
            ["Quick", "O(n log n)", "O(n log n)", "O(n^2)", "No", "In-place average speed"],
            ["Heap", "O(n log n)", "O(n log n)", "O(n log n)", "No", "Memory-sensitive sorting"],
        ]),
        ("Tree Traversal Table", ["Traversal", "Order", "Use"], [
            ["Preorder", "Root, left, right", "Copy/serialize structure"],
            ["Inorder", "Left, root, right", "BST sorted order"],
            ["Postorder", "Left, right, root", "Delete/free or compute child facts first"],
            ["Level order", "Breadth first", "Shortest layers and views"],
        ]),
        ("Graph Algorithm Comparison", ["Algorithm", "Graph type", "Purpose"], [
            ["BFS", "Unweighted", "Shortest edge count"],
            ["Dijkstra", "Nonnegative weighted", "Shortest weighted path"],
            ["Bellman-Ford", "Negative edges", "Shortest path and negative-cycle detection"],
            ["Kruskal/Prim", "Undirected weighted", "Minimum spanning tree"],
            ["Kosaraju/Tarjan", "Directed", "Strongly connected components"],
        ]),
        ("DP Pattern Comparison", ["Pattern", "State", "Typical transition"], [
            ["Knapsack", "i, capacity", "take or skip"],
            ["Grid", "row, col", "from top/left or neighbors"],
            ["LCS", "i, j", "match or drop one side"],
            ["LIS", "i or tails length", "extend smaller tail"],
            ["Interval", "l, r", "try split point"],
        ]),
        ("Java Collections Comparison", ["Type", "Ordered?", "Duplicates?", "Primary method"], [
            ["HashSet", "No", "No", "contains/add/remove"],
            ["TreeSet", "Sorted", "No", "floor/ceiling"],
            ["HashMap", "No", "Keys unique", "get/put"],
            ["TreeMap", "Sorted keys", "Keys unique", "floorKey/ceilingKey"],
            ["PriorityQueue", "Heap order", "Yes", "offer/poll/peek"],
        ]),
        ("Common Formula Sheet", ["Formula", "Use"], [
            ["sum 1..n = n(n+1)/2", "Counting pairs, arithmetic ranges"],
            ["subarrays in length n = n(n+1)/2", "Counting contiguous choices"],
            ["subsets = 2^n", "Bitmask/backtracking"],
            ["permutations = n!", "Ordering all elements"],
            ["edges in complete graph = n(n-1)/2", "Dense graph bounds"],
        ]),
        ("Bit Manipulation Tricks", ["Expression", "Meaning"], [
            ["x & -x", "Lowest set bit"],
            ["x & (x - 1)", "Remove lowest set bit"],
            ["x ^ x = 0", "Pair cancellation"],
            ["mask | (1 << b)", "Set bit b"],
            ["mask & ~(1 << b)", "Clear bit b"],
        ]),
    ]

    for title, headers, rows in appendix_tables:
        doc.add_heading(title, level=1)
        widths = [PAGE_WIDTH_DXA // len(headers)] * len(headers)
        widths[-1] += PAGE_WIDTH_DXA - sum(widths)
        add_table(doc, headers, rows, widths, header_fill=LIGHT_TEAL)
        doc.add_page_break()

    templates = [
        ("Recursion Template", CODE["Recursion"]),
        ("DFS Template", CODE["Graphs"]),
        ("BFS Template", CODE["Graphs"]),
        ("Binary Search Template", CODE["Binary Search"]),
        ("Union Find Template", CODE["Disjoint Set Union"]),
        ("Segment Tree Template", CODE["Segment Tree"]),
        ("Trie Template", CODE["Trie"]),
        ("Monotonic Stack Template", CODE["Monotonic Stack"]),
        ("Sliding Window Template", CODE["Sliding Window"]),
        ("Two Pointer Template", CODE["Two Pointer"]),
        ("Fast Input Template", CODE["Programming Basics"]),
        ("Competitive Programming Utilities", CODE["Mathematics"]),
    ]
    for idx, (title, code) in enumerate(templates):
        doc.add_heading(title, level=1)
        add_code(doc, code)
        add_callout(doc, "Usage", "Rewrite this template from memory at least twice. In interviews, templates are only useful when you understand the invariant behind each line.", "tip")
        if idx != len(templates) - 1:
            doc.add_page_break()


def build() -> None:
    doc = Document()
    setup_styles(doc)
    setup_header_footer(doc)
    props = doc.core_properties
    props.title = "Striver's A2Z DSA Handbook"
    props.subject = "Interview-focused Data Structures and Algorithms handbook"
    props.author = "Codex"
    props.comments = "Original educational handbook generated for interview preparation."

    add_cover(doc)
    add_front_matter(doc)
    add_foundation_overview(doc)
    add_complexity_cheat(doc)
    add_java_collections(doc)

    current_part = None
    for idx, topic in enumerate(TOPICS, start=1):
        if topic["part"] != current_part:
            current_part = topic["part"]
            doc.add_heading(current_part, level=1)
            add_para(doc, f"This part groups the {current_part.lower()} topics from the A2Z preparation path. Read the chapters in order once, then revisit them by pattern when solving mixed problems.")
            doc.add_page_break()
        add_topic(doc, idx, topic)

    add_pattern_handbooks(doc)
    add_strategy_sections(doc)
    add_revision_plans(doc)
    add_appendix(doc)

    # Clean trailing blank page if the final template inserted one.
    if doc.paragraphs and not doc.paragraphs[-1].text:
        pass

    doc.save(OUT)
    print(OUT)


if __name__ == "__main__":
    build()
