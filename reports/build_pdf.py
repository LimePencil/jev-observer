#!/usr/bin/env python3
"""Build the dated project guide and 100-project appendix from editable sources."""
import csv
from html import escape
import json
from pathlib import Path
import re

from reportlab.lib import colors
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.platypus import (
    BaseDocTemplate, Flowable, Frame, KeepTogether, PageBreak, PageTemplate,
    Paragraph, Spacer, Table, TableStyle,
)

ROOT = Path(__file__).resolve().parents[1]
HERE = Path(__file__).resolve().parent
WIDTH, HEIGHT = A4
MARGIN = 48
CONTENT = WIDTH - 2 * MARGIN
INK = colors.HexColor('#172B3A')
TEAL = colors.HexColor('#087F8C')
MUTED = colors.HexColor('#536676')
PALE = colors.HexColor('#EEF5F7')
LINE = colors.HexColor('#D8E4E9')

FONT_DIR = Path('/usr/share/fonts/truetype/dejavu')
for name, filename in [
    ('Guide', 'DejaVuSans.ttf'), ('Guide-Bold', 'DejaVuSans-Bold.ttf'),
]:
    pdfmetrics.registerFont(TTFont(name, str(FONT_DIR / filename)))
pdfmetrics.registerFontFamily('Guide', normal='Guide', bold='Guide-Bold', italic='Guide', boldItalic='Guide-Bold')

STYLES = {
    'body': ParagraphStyle('Body', fontName='Guide', fontSize=9.3, leading=13.4,
                           textColor=INK, spaceAfter=8),
    'title': ParagraphStyle('Title', fontName='Guide-Bold', fontSize=31, leading=38,
                            textColor=INK, spaceBefore=42, spaceAfter=18),
    'h1': ParagraphStyle('H1', fontName='Guide-Bold', fontSize=20, leading=25,
                         textColor=INK, spaceAfter=16, keepWithNext=True),
    'h2': ParagraphStyle('H2', fontName='Guide-Bold', fontSize=11, leading=15,
                         textColor=TEAL, spaceBefore=10, spaceAfter=7, keepWithNext=True),
    'cell': ParagraphStyle('Cell', fontName='Guide', fontSize=8.2, leading=11.3, textColor=INK),
    'th': ParagraphStyle('TH', fontName='Guide-Bold', fontSize=8.2, leading=11.3, textColor=colors.white),
    'bullet': ParagraphStyle('Bullet', fontName='Guide', fontSize=9.2, leading=13,
                             textColor=INK, leftIndent=12, firstLineIndent=-10, spaceAfter=6),
    'card_title': ParagraphStyle('CardTitle', fontName='Guide-Bold', fontSize=10.2, leading=14,
                                 textColor=INK, spaceAfter=4),
    'card': ParagraphStyle('Card', fontName='Guide', fontSize=8.1, leading=11.2,
                           textColor=INK, spaceAfter=3),
    'small': ParagraphStyle('Small', fontName='Guide', fontSize=7.5, leading=10.4,
                            textColor=MUTED, spaceAfter=5),
}


def inline(text):
    text = escape(text)
    text = re.sub(r'\[([^\]]+)\]\((https?://[^)]+)\)',
                  lambda m: f'<link href="{m[2]}" color="#087F8C">{m[1]}</link>', text)
    text = re.sub(r'\*\*(.+?)\*\*', r'<b>\1</b>', text)
    return text


def para(text, style='body'):
    return Paragraph(inline(text), STYLES[style])


class Bars(Flowable):
    def __init__(self, counts):
        super().__init__()
        self.counts = counts
        self.width, self.height = CONTENT, 142

    def draw(self):
        rows = [('fixed', 'Fixed recurring definitions'), ('rules', 'Configured rules'),
                ('candidates', 'Changing candidate sets'), ('indexed', 'Indexed/contextual instances'),
                ('study', 'Study/benchmark variants')]
        for index, (key, label) in enumerate(rows):
            y = 119 - index * 25
            self.canv.setFont('Guide', 8.8)
            self.canv.setFillColor(INK)
            self.canv.drawString(0, y + 3, label)
            self.canv.setFillColor(PALE)
            self.canv.roundRect(192, y, 251, 15, 3, fill=1, stroke=0)
            self.canv.setFillColor(TEAL)
            self.canv.roundRect(192, y, 251 * self.counts[key] / 30, 15, 3, fill=1, stroke=0)
            self.canv.setFillColor(INK)
            self.canv.setFont('Guide-Bold', 9)
            self.canv.drawString(454, y + 3, str(self.counts[key]))


class Architecture(Flowable):
    def __init__(self):
        super().__init__()
        self.width, self.height = CONTENT, 139

    def box(self, x, y, width, label):
        self.canv.setFillColor(PALE)
        self.canv.setStrokeColor(LINE)
        self.canv.roundRect(x, y, width, 34, 5, fill=1, stroke=1)
        self.canv.setFillColor(INK)
        self.canv.setFont('Guide-Bold', 8)
        self.canv.drawCentredString(x + width / 2, y + 13, label)

    def arrow(self, x1, y1, x2, y2):
        self.canv.setStrokeColor(TEAL)
        self.canv.setLineWidth(1)
        self.canv.line(x1, y1, x2, y2)
        if y1 == y2:
            self.canv.line(x2 - 4, y2 - 3, x2, y2)
            self.canv.line(x2 - 4, y2 + 3, x2, y2)
        else:
            self.canv.line(x2 - 3, y2 + 4, x2, y2)
            self.canv.line(x2 + 3, y2 + 4, x2, y2)

    def draw(self):
        self.box(0, 88, 118, 'Local application')
        self.box(184, 88, 131, 'Observer proxy')
        self.box(380, 88, 119, 'Jev provider')
        self.arrow(118, 105, 184, 105)
        self.arrow(315, 105, 380, 105)
        self.box(0, 11, 118, 'Local file import')
        self.box(184, 11, 131, 'Queue + local store')
        self.box(380, 11, 119, 'Browser explorer')
        self.arrow(118, 28, 184, 28)
        self.arrow(315, 28, 380, 28)
        self.arrow(249, 88, 249, 45)
        self.canv.setFont('Guide', 7)
        self.canv.setFillColor(MUTED)
        self.canv.drawString(260, 63, 'bounded capture')


class GuideDoc(BaseDocTemplate):
    def afterFlowable(self, flowable):
        if isinstance(flowable, Paragraph) and flowable.style.name in ('H1', 'Title'):
            label = flowable.getPlainText()
            key = f'heading-{self.page}'
            self.canv.bookmarkPage(key)
            self.canv.addOutlineEntry(label, key, 0, False)


def decorate(canvas, doc):
    canvas.saveState()
    canvas.setFillColor(TEAL)
    canvas.rect(0, HEIGHT - 9, WIDTH, 9, fill=1, stroke=0)
    canvas.setFont('Guide-Bold', 7)
    canvas.setFillColor(MUTED)
    canvas.drawString(MARGIN, HEIGHT - 31, 'JEV OBSERVER  /  PROJECT START GUIDE')
    canvas.setFont('Guide', 7)
    canvas.drawRightString(WIDTH - MARGIN, HEIGHT - 31, 'RESEARCH SNAPSHOT · 22 SEP 2026')
    canvas.setStrokeColor(LINE)
    canvas.line(MARGIN, 40, WIDTH - MARGIN, 40)
    canvas.setFont('Guide', 7)
    canvas.drawString(MARGIN, 27, 'Free · Local · MIT   |   Proposed product; integrations unverified')
    canvas.drawRightString(WIDTH - MARGIN, 27, str(doc.page))
    canvas.restoreState()


def make_table(lines):
    cells = [[c.strip() for c in line.strip().strip('|').split('|')] for line in lines]
    cells = [row for row in cells if not all(re.fullmatch(r'[-:]+', cell) for cell in row)]
    count = len(cells[0])
    widths = [CONTENT * 0.29, CONTENT * 0.71] if count == 2 else [CONTENT * 0.26, CONTENT * 0.37, CONTENT * 0.37]
    if count != len(widths):
        widths = [CONTENT / count] * count
    data = [[para(cell, 'th' if i == 0 else 'cell') for cell in row] for i, row in enumerate(cells)]
    table = Table(data, colWidths=widths, hAlign='LEFT', repeatRows=1)
    table.setStyle(TableStyle([
        ('BACKGROUND', (0, 0), (-1, 0), INK),
        ('ROWBACKGROUNDS', (0, 1), (-1, -1), [PALE, colors.white]),
        ('VALIGN', (0, 0), (-1, -1), 'TOP'),
        ('LEFTPADDING', (0, 0), (-1, -1), 8), ('RIGHTPADDING', (0, 0), (-1, -1), 8),
        ('TOPPADDING', (0, 0), (-1, -1), 7), ('BOTTOMPADDING', (0, 0), (-1, -1), 7),
        ('LINEBELOW', (0, -1), (-1, -1), 0.5, LINE),
    ]))
    return table


def main():
    counts = json.loads((ROOT / 'research/survey/counts.json').read_text())['primary_patterns']
    story = []
    blocks = (HERE / 'project-start-guide.md').read_text().split('<!-- page -->')
    for block_index, block in enumerate(blocks):
        if block_index:
            story.append(PageBreak())
        lines = block.strip().splitlines()
        index = 0
        while index < len(lines):
            line = lines[index].strip()
            if not line:
                index += 1
                continue
            if line.startswith('|'):
                table_lines = []
                while index < len(lines) and lines[index].strip().startswith('|'):
                    table_lines.append(lines[index]); index += 1
                story.extend([make_table(table_lines), Spacer(1, 10)])
                continue
            if line == '[PATTERN_CHART]':
                story.append(Bars(counts))
            elif line == '[ARCHITECTURE]':
                story.append(Architecture())
            elif line.startswith('# '):
                story.append(para(line[2:], 'title' if block_index == 0 else 'h1'))
            elif line.startswith('## '):
                story.append(para(line[3:], 'h2'))
            elif line.startswith('- '):
                story.append(para('• ' + line[2:], 'bullet'))
            elif re.match(r'^\d+\. ', line):
                story.append(para(line, 'bullet'))
            else:
                paragraph = [line]
                while index + 1 < len(lines) and lines[index + 1].strip() and not lines[index + 1].startswith(('#', '|', '- ')):
                    index += 1; paragraph.append(lines[index].strip())
                story.append(para(' '.join(paragraph)))
            index += 1

    with (ROOT / 'research/survey/projects.csv').open() as stream:
        projects = list(csv.DictReader(stream))
    pattern_names = {'fixed': 'Fixed definitions', 'rules': 'Configured rules', 'indexed': 'Indexed/contextual instances',
                     'candidates': 'Changing candidates', 'study': 'Study/benchmark variants'}
    for start in range(0, len(projects), 4):
        story.append(PageBreak())
        story.append(para(f'Appendix / Projects {start + 1:03}–{start + 4:03}', 'h1'))
        story.append(para('Static review · Proposed benefits are hypotheses · Code and README links are pinned to reviewed commits.', 'small'))
        for row in projects[start:start + 4]:
            header = para(f'{int(row["id"]):03}  {row["repository"]}', 'card_title')
            sub = para(f'{row["category"]}  /  {pattern_names[row["primary_pattern"]]}', 'small')
            items = [header, sub]
            for label, field in [('Use', 'observed_use'), ('Already visible', 'existing_visibility'),
                                 ('Observer could help', 'proposed_help'), ('Limit', 'integration_limit')]:
                items.append(para(f'**{label}:** {row[field]}', 'card'))
            items.append(para(f'[Implementation evidence]({row["evidence_url"]})  ·  [Project README]({row["readme_url"]})', 'small'))
            items.append(Spacer(1, 11))
            story.append(KeepTogether(items))

    destination = HERE / 'jev-observer-project-start-guide.pdf'
    doc = GuideDoc(str(destination), pagesize=A4, leftMargin=MARGIN, rightMargin=MARGIN,
                   topMargin=54, bottomMargin=52, title='Jev Observer — Project Start Guide',
                   author='Jev Observer project research', subject='Free local Jev observability: product, architecture and 100-project survey')
    frame = Frame(MARGIN, 52, CONTENT, HEIGHT - 106, leftPadding=0, rightPadding=0, topPadding=0, bottomPadding=0)
    doc.addPageTemplates(PageTemplate(id='guide', frames=[frame], onPage=decorate))
    doc.build(story)
    print(destination)


if __name__ == '__main__':
    main()
