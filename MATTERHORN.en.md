# Mimi and the Matterhorn Protocol

[Deutsch](MATTERHORN.md) · **English**

*Mimi – Macht Inhalte Mühelos Inklusiv*

The **Matterhorn Protocol** (PDF Association, version 1.1) describes in 31 checkpoints how
a PDF fails to conform to PDF/UA-1 (ISO 14289-1). Some of the failure conditions can be
checked by software (veraPDF, PAC), the rest only by humans.

This overview shows for each checkpoint what Mimi does and what remains with a human.
Status: November 2026, Mimi with LibreOffice 26.8 and veraPDF 1.30.

## Legend

| Rating | Meaning |
|---|---|
| **yes** | Mimi detects and fixes this (in the Word file, the export copy or the finished PDF) |
| **partly** | Mimi fixes some of it, gives hints or only checks; the rest is manual work |
| **no** | not covered, manual work only |
| **n/a** | not applicable to reports from Word (the feature does not occur in the generated PDF) |

**Where Mimi intervenes:**

| Stage | What |
|---|---|
| **Analysis** | findings in the Word file, fixed with a click or with a hint to fix it in Word |
| **Preparation** | automatic adjustments to an export copy for LibreOffice (the Word file stays unchanged) |
| **Post-processing** | corrections to the structure tree of the finished PDF |
| **Basic check** | Mimi's own check of the PDF |
| **veraPDF** | complete automated validation against PDF/UA-1 |

## Overview

| No. | Checkpoint | Mimi | What Mimi does | Remains with a human |
|---|---|---|---|---|
| 01 | Real content tagged, artifacts as artifacts | **partly** | LibreOffice tags the content, headers and footers become artifacts. Preparation: links and fields in headers/footers become text, images and shapes there become decorative. Post-processing: shapes marked as decorative become artifacts. veraPDF checks for untagged content. | Is anything wrongly an artifact, or is decoration tagged as content? |
| 02 | Role mapping | **yes** | Preparation: custom styles named after a standard tag (“TD”, “Quote” …) are renamed. Basic check “Standard-Tags umgeleitet” (standard tags remapped), veraPDF. | – |
| 03 | Flickering | **n/a** | Reports from Word contain no animations. | – |
| 04 | Color and contrast | **partly** | Information conveyed only by colour is **not** detected. Additionally (WCAG 1.4.3, not Matterhorn): contrast of font colours against white, fixed by darkening. | Is information conveyed only by colour, shape or position? Contrast on coloured backgrounds and in graphics. |
| 05 | Sound | **n/a** | – | – |
| 06 | Metadata | **yes** | Analysis: the document title is asked for and set. LibreOffice writes XMP metadata and the PDF/UA identifier. Basic check: XMP metadata, PDF/UA identifier, document title. | Does the title really describe the document? |
| 07 | Dictionary (title display) | **yes** | LibreOffice sets `DisplayDocTitle`. Basic check “Fenster zeigt Dateinamen statt Titel” (window shows file name instead of title). | – |
| 08 | OCR validation | **n/a** | Mimi processes Word files, not scans. | – |
| 09 | Appropriate tags | **partly** | Post-processing: divisions, paragraphs and headings that LibreOffice puts inside a paragraph (Word frames, text boxes) are moved out (basic check “Falsch verschachtelte Struktur”, incorrectly nested structure). Preparation: captions become paragraphs instead of free-standing `Caption`; the bibliography becomes text instead of `TOC`. Analysis: hints on bold pseudo-headings, typed lists, empty paragraphs used as spacing, single-row tables (layout). | **Reading order** (multi-column, text boxes, floating images) and whether the tags convey the meaning correctly. |
| 10 | Character mappings (Unicode) | **partly** | No correction of its own. LibreOffice embeds the fonts with Unicode mapping. veraPDF checks this. | If problems occur: install the font (see INSTALL.en.md, Fonts). |
| 11 | Declared natural language | **partly** | Analysis: the main language is asked for and set. Basic check “Dokumentsprache fehlt” (document language missing). LibreOffice only takes over language changes in the text if they are set in Word. | Mark **passages in other languages** in Word. Mimi does not detect them. |
| 12 | Stretchable characters | **n/a** | Concerns scripts such as Arabic or Japanese. | – |
| 13 | Graphics | **partly** | Analysis: missing and meaningless alt texts (“Bild 3”, file name), taking over the caption, “decorative”. Hints on vector graphics and SmartArt with text and on tables pasted as images. Preparation: graphics in headers/footers become decorative. Basic check “Abbildungen ohne Alternativtext” (figures without alt text). | **Quality of the alt texts.** Text in graphics (image instead of a real table or real text). Groups of graphics that only make sense together. |
| 14 | Headings | **yes** | Analysis: skipped levels are fixed with a click. Hints: no headings, first heading not level 1, bold instead of heading. Basic check: skipped heading level, no headings. | Is everything that looks like a heading actually one? |
| 15 | Tables | **partly** | Analysis: mark the first row as header row. Basic check “Tabellen ohne Kopfzellen” (tables without header cells), veraPDF. **Not:** row headers (first column), multi-level headers, `Headers`/`IDs`. LibreOffice sets `Scope /Column` on header cells and `RowSpan`/`ColSpan` on merged cells, also with two-level headers (verified on the test report). | Prefer simple tables. Check complex tables and header columns. Avoid layout tables. |
| 16 | Lists | **partly** | LibreOffice tags Word lists as lists, veraPDF checks the structure. Analysis: hint on typed lists. Correctly nested in the test (`L`/`LI`/`Lbl`/`LBody`, including sub-lists). Cosmetic issues: bullet characters are missing from `Lbl`, `ListNumbering` is missing for “a) b)” and bullets (only required by PDF/UA-2). | Use only Word's list function for lists. |
| 17 | Mathematical expressions | **partly** | Analysis: equations from Word's equation editor are listed, with the linear format as a suggestion (e.g. `E=mc^2`) and an own text. Post-processing: the text is set as `Alt` on `Formula` (only if the number of equations in Word and PDF matches). Basic check “Formeln ohne Alternativtext” (equations without alt text). No MathML (only PDF/UA-2 provides for it). | Is the text read out understandably? Spell out complex equations. |
| 18 | Page headers and footers | **yes** | LibreOffice marks them as artifacts. Preparation: links and reference fields (`STYLEREF`, `PAGEREF` …) become text, images and shapes become decorative. | – |
| 19 | Notes and references | **partly** | No correction of its own. LibreOffice tags footnotes and endnotes as `Note` with a unique `ID`, with number (`Lbl`) and links both ways (verified on the test report and on the 6 footnotes of the Praxistest report from Word). veraPDF checks the `ID`. In the reading order the notes come at the end of the document. | – |
| 20 | Optional content (layers) | **n/a** | Does not occur in Word reports. | – |
| 21 | Embedded files | **n/a** | LibreOffice does not embed files. | – |
| 22 | Article threads | **n/a** | – | – |
| 23 | Digital signatures | **n/a** | Mimi does not sign. | – |
| 24 | Non-interactive forms | **n/a** | Forms are not supported. | Do not create forms with Mimi. |
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
| **partly** or checked only | 01, 04, 09, 10, 11, 13, 15, 16, 17, 19, 30 (11) |
| **no**, not covered | – (0) |
| **n/a**, not applicable to Word reports | 03, 05, 08, 12, 20–26, 29 (12) |

**Result of the field test** (Praxistest final report, 100 pages): after preparation and
corrections in Mimi, veraPDF reports no more violations of PDF/UA-1, see
[FUNCTION.en.md](FUNCTION.en.md#field-test-Praxistest-final-report-november-2026).

## What humans always have to check

No software can judge:

1. whether the **alt texts** convey the content and purpose of the images,
2. whether the **reading order** is right, especially with text boxes, frames and multi-column layout,
3. whether information is conveyed **only by colour or position**,
4. whether **tables** are structured understandably,
5. whether **passages in other languages** are marked,
6. whether **link texts** are understandable.

Mimi requires a decision for every image (text or decorative). Afterwards the PDF is checked
with PAC and ideally also with a screen reader (e.g. NVDA).

## Verified on a test report (November 2026)

A test report with footnotes, an endnote, an equation, a multi-level numbered list, a table
with a two-level header and merged cells, and a two-column section went through Mimi
(analysis, preparation, LibreOffice 26.8, post-processing):

| Case | Result in the PDF |
|---|---|
| Footnotes and endnotes | `Note` with unique `ID`, number as `Lbl`, links both ways |
| Lists | `L`/`LI`/`Lbl`/`LBody`, sub-list correctly nested |
| Table | `TH` with `Scope /Column`, `RowSpan`/`ColSpan` |
| Two columns | reading order correct: left column, right column, then the text below |
| Equation | before equation support the only violation (veraPDF 7.7-1), afterwards `Alt` is set |

**veraPDF (PDF/UA-1): compliant.**

The test report was created with LibreOffice. That is why a real report from Microsoft Word
also went through Mimi.

## Field test with the Praxistest final report (Word, November 2026)

100 pages, 64 images, 12 tables, 6 footnotes, 3 Word frames, no equations. Through Mimi with all
corrections that can be clicked (the images' alt texts were placeholders):

| Step | Result |
|---|---|
| Analysis | 64 images, 12 tables, 4 skipped heading levels, 113 links without description |
| Preparation | style “TD” renamed, captions as paragraphs, spacing in 424 table cells, 8 links in headers/footers, 10 graphics in headers/footers decorative |
| Post-processing | 3 divisions moved out of paragraphs, 7 decorative shapes turned into artifacts |
| Footnotes | all 6 as `Note` with unique `ID`, number and link |
| Basic check | nothing open |
| **veraPDF (PDF/UA-1)** | **compliant** |

**As ODT:** the same report, saved as OpenDocument in LibreOffice and uploaded that way, is also
**compliant** after Mimi (100 pages, veraPDF PDF/UA-1). Exported directly from LibreOffice it
violates 8 rules. The test report with special cases is compliant as ODT too.

**Still open:** equations in a file straight from Microsoft Word (the Praxistest report contains none).

## Outlook: PDF/UA-2

The same Praxistest report, additionally checked with veraPDF against PDF/UA-2 (ISO 14289-2):

| Rule | Count | What is missing |
|---|---|---|
| 6.1-1, 8.2.5.2-2, 5-2, 5-5 | 1 each | file version PDF 2.0, PDF 2.0 namespace, identifier “PDF/UA-2” |
| 8.2.5.14-1 | 6 | in PDF 2.0 footnotes are called `FENote` instead of `Note` |
| 8.2.5.8-1 | 111 | entries in tables of contents need a reference (`Ref`) to their target |
| 8.8-1 | 304 | links within the document must point to the structure element, not to a position on the page |
| 8.2.5.25-1 | 2 | `ListNumbering` on lists |

These are almost all format questions of PDF 2.0, not content defects. LibreOffice produces
PDF 1.7 according to PDF/UA-1. PDF/UA-2 requires LibreOffice to write PDF 2.0 or Mimi to
restructure the file extensively. That is why PDF/UA-1 is Mimi's current target. It is also
what PAC and most testing bodies expect.

## Mimi's user interface

Mimi itself is meant to be accessible: labelled form fields, error messages as
`role="alert"`, status messages via `aria-live`, visible focus, sufficient contrast
(also in dark mode). The tabs follow the ARIA “Tabs” pattern and can be operated with
arrow left/right, Home and End.

Not yet systematically tested: screen readers (NVDA, JAWS, VoiceOver) and zoom to
200–400 %.
