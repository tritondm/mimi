# Leo and the Matterhorn Protocol

[Deutsch](MATTERHORN.md) · **English**

*Leo – Leichte Erstellung Ohne Barrieren*

The **Matterhorn Protocol** (PDF Association, version 1.1) describes in 31 checkpoints how
a PDF fails to conform to PDF/UA-1 (ISO 14289-1). Some of the failure conditions can be
checked by software (veraPDF, PAC), the rest only by humans.

This overview shows for each checkpoint what Leo does and what remains with a human.
Status: November 2026, Leo with LibreOffice 26.8 and veraPDF 1.30.

## Legend

| Rating | Meaning |
|---|---|
| **yes** | Leo detects and fixes this (in the Word file, the export copy or the finished PDF) |
| **partly** | Leo fixes some of it, gives hints or only checks; the rest is manual work |
| **no** | not covered, manual work only |
| **n/a** | not applicable to reports from Word (the feature does not occur in the generated PDF) |

**Where Leo intervenes:**

| Stage | What |
|---|---|
| **Analysis** | findings in the Word file, fixed with a click or with a hint to fix it in Word |
| **Preparation** | automatic adjustments to an export copy for LibreOffice (the Word file stays unchanged) |
| **Post-processing** | corrections to the structure tree of the finished PDF |
| **Basic check** | Leo's own check of the PDF |
| **veraPDF** | complete automated validation against PDF/UA-1 |

## Overview

| No. | Checkpoint | Leo | What Leo does | Remains with a human |
|---|---|---|---|---|
| 01 | Real content tagged, artifacts as artifacts | **partly** | LibreOffice tags the content, headers and footers become artifacts. Preparation: links and fields in headers/footers become text, images and shapes there become decorative. Post-processing: shapes marked as decorative become artifacts. veraPDF checks for untagged content. | Is anything wrongly an artifact, or is decoration tagged as content? |
| 02 | Role mapping | **yes** | Preparation: custom styles named after a standard tag (“TD”, “Quote” …) are renamed. Basic check “Standard-Tags umgeleitet” (standard tags remapped), veraPDF. | – |
| 03 | Flickering | **n/a** | Reports from Word contain no animations. | – |
| 04 | Color and contrast | **partly** | Information conveyed only by colour is **not** detected. Additionally (WCAG 1.4.3, not Matterhorn): contrast of font colours against white, fixed by darkening. | Is information conveyed only by colour, shape or position? Contrast on coloured backgrounds and in graphics. |
| 05 | Sound | **n/a** | – | – |
| 06 | Metadata | **yes** | Analysis: the document title is asked for and set. LibreOffice writes XMP metadata and the PDF/UA identifier. Basic check: XMP metadata, PDF/UA identifier, document title. | Does the title really describe the document? |
| 07 | Dictionary (title display) | **yes** | LibreOffice sets `DisplayDocTitle`. Basic check “Fenster zeigt Dateinamen statt Titel” (window shows file name instead of title). | – |
| 08 | OCR validation | **n/a** | Leo processes Word files, not scans. | – |
| 09 | Appropriate tags | **partly** | Post-processing: divisions, paragraphs and headings that LibreOffice puts inside a paragraph (Word frames, text boxes) are moved out (basic check “Falsch verschachtelte Struktur”, incorrectly nested structure). Preparation: captions become paragraphs instead of free-standing `Caption`; the bibliography becomes text instead of `TOC`. Analysis: hints on bold pseudo-headings, typed lists, empty paragraphs used as spacing, single-row tables (layout). | **Reading order** (multi-column, text boxes, floating images) and whether the tags convey the meaning correctly. |
| 10 | Character mappings (Unicode) | **partly** | No correction of its own. LibreOffice embeds the fonts with Unicode mapping. veraPDF checks this. | If problems occur: install the font (see INSTALL.en.md, Fonts). |
| 11 | Declared natural language | **partly** | Analysis: the main language is asked for and set. Basic check “Dokumentsprache fehlt” (document language missing). LibreOffice only takes over language changes in the text if they are set in Word. | Mark **passages in other languages** in Word. Leo does not detect them. |
| 12 | Stretchable characters | **n/a** | Concerns scripts such as Arabic or Japanese. | – |
| 13 | Graphics | **partly** | Analysis: missing and meaningless alt texts (“Bild 3”, file name), taking over the caption, “decorative”. Hints on vector graphics and SmartArt with text and on tables pasted as images. Preparation: graphics in headers/footers become decorative. Basic check “Abbildungen ohne Alternativtext” (figures without alt text). | **Quality of the alt texts.** Text in graphics (image instead of a real table or real text). Groups of graphics that only make sense together. |
| 14 | Headings | **yes** | Analysis: skipped levels are fixed with a click. Hints: no headings, first heading not level 1, bold instead of heading. Basic check: skipped heading level, no headings. | Is everything that looks like a heading actually one? |
| 15 | Tables | **partly** | Analysis: mark the first row as header row. Basic check “Tabellen ohne Kopfzellen” (tables without header cells), veraPDF. **Not:** row headers (first column), multi-level headers, merged cells, `Headers`/`IDs`. Whether LibreOffice sets `Scope` has not been verified. | Prefer simple tables. Check complex tables and header columns. Avoid layout tables. |
| 16 | Lists | **partly** | LibreOffice tags Word lists as lists, veraPDF checks the structure. Analysis: hint on typed lists. Whether `Lbl` and `LBody` are always correct has not been verified. | Use only Word's list function for lists. |
| 17 | Mathematical expressions | **no** | Word equations are neither detected nor given alt text. | Check equations or post-process them in Acrobat. |
| 18 | Page headers and footers | **yes** | LibreOffice marks them as artifacts. Preparation: links and reference fields (`STYLEREF`, `PAGEREF` …) become text, images and shapes become decorative. | – |
| 19 | Notes and references | **no** | No check of its own. Whether LibreOffice tags footnotes as `Note` with a unique `ID` has not been verified (veraPDF checks the `ID`). | Check a report with footnotes using PAC and a screen reader. |
| 20 | Optional content (layers) | **n/a** | Does not occur in Word reports. | – |
| 21 | Embedded files | **n/a** | LibreOffice does not embed files. | – |
| 22 | Article threads | **n/a** | – | – |
| 23 | Digital signatures | **n/a** | Leo does not sign. | – |
| 24 | Non-interactive forms | **n/a** | Forms are not supported. | Do not create forms with Leo. |
| 25 | XFA | **n/a** | – | – |
| 26 | Security | **n/a** | LibreOffice does not encrypt; veraPDF checks that assistive technology has access. | Do not add permission restrictions to the PDF afterwards. |
| 27 | Navigation | **yes** | Headings become bookmarks. Table of contents and lists of figures and tables stay linked (LibreOffice 26.8). Basic check “Keine Überschriften” (no headings). | – |
| 28 | Annotations (links) | **yes** | Analysis: missing link descriptions are added from the link text, links to local files are reported; hint on bare web addresses. Preparation: links in headers/footers and to local files become text. Basic check: untagged links, links without description. | Are the link texts understandable? |
| 29 | Actions | **n/a** | No JavaScript, no form actions. | – |
| 30 | XObjects | **partly** | No correction of its own. Images and shapes are XObjects from LibreOffice, veraPDF checks them. Reference XObjects do not occur. | – |
| 31 | Fonts | **yes** | Fonts are installed in the container (incl. Carlito for Calibri). Basic check “Nicht eingebettete Schriften” (fonts not embedded), veraPDF checks the rest. | Install corporate fonts without a substitute (e.g. Aptos, Segoe UI), see INSTALL.en.md. |

## Summary

| | Checkpoints |
|---|---|
| **yes**, covered | 02, 06, 07, 14, 18, 27, 28, 31 (8) |
| **partly** or checked only | 01, 04, 09, 10, 11, 13, 15, 16, 30 (9) |
| **no**, not covered | 17, 19 (2) |
| **n/a**, not applicable to Word reports | 03, 05, 08, 12, 20–26, 29 (12) |

**Result of the field test** (Praxistest final report, 100 pages): after preparation and
corrections in Leo, veraPDF reports no more violations of PDF/UA-1, see
[FUNCTION.en.md](FUNCTION.en.md#field-test-Praxistest-final-report-november-2026).

## What humans always have to check

No software can judge:

1. whether the **alt texts** convey the content and purpose of the images,
2. whether the **reading order** is right, especially with text boxes, frames and multi-column layout,
3. whether information is conveyed **only by colour or position**,
4. whether **tables** are structured understandably,
5. whether **passages in other languages** are marked,
6. whether **link texts** are understandable.

Leo requires a decision for every image (text or decorative). Afterwards the PDF is checked
with PAC and ideally also with a screen reader (e.g. NVDA).

## Still to be verified (open questions)

These items are not handled specifically in the code. How LibreOffice implements them has
not yet been verified on real reports:

- footnotes and endnotes: `Note` tag, unique `ID`, reference back
- tables: `Scope` attribute on header cells, header columns, merged cells
- lists: `Lbl`/`LBody` in multi-level and numbered lists
- Word equations

## Leo's user interface

Leo itself is meant to be accessible: labelled form fields, error messages as
`role="alert"`, status messages via `aria-live`, visible focus, sufficient contrast
(also in dark mode). The tabs follow the ARIA “Tabs” pattern and can be operated with
arrow left/right, Home and End.

Not yet systematically tested: screen readers (NVDA, JAWS, VoiceOver) and zoom to
200–400 %.
