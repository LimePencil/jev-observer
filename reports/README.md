# Project start guide

[Download the PDF](jev-observer-project-start-guide.pdf). It combines the product/build guide with all 100 project assessments and clickable source links. This is a dated research/design document, not a claim that the proposed application exists.

Editable content is in [project-start-guide.md](project-start-guide.md). The appendix is generated from [the survey CSV](../research/survey/projects.csv), itself generated from manual annotations and pinned source metadata.

## Rebuild

Use Python 3, the DejaVu Sans fonts at `/usr/share/fonts/truetype/dejavu`, and the pinned PDF dependencies. A temporary environment keeps reporting dependencies separate from any future application runtime:

```sh
uv venv /tmp/jev-pdf-tools
uv pip install --python /tmp/jev-pdf-tools/bin/python -r reports/requirements.txt
python3 research/survey/scripts/build_report.py
/tmp/jev-pdf-tools/bin/python reports/build_pdf.py
```

The generator uses ReportLab and embeds fonts. The PDF contains selectable text, source hyperlinks, page numbers, a reading map and navigation bookmarks. `pypdf` is used for verification. No studied third-party application is installed or executed during generation.
