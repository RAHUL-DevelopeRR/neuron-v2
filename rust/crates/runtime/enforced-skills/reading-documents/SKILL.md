---
name: reading-documents
description: Read Word, PowerPoint, Excel and PDF files with read_document
license: MIT
---
# Reading documents

Neuron reads .docx, .pptx, .xlsx, .xlsm, .xlsb, .xls, .ods and .pdf files itself.

- Call `read_document` with the file path. It returns Markdown: headings, lists and tables, each slide with its title and speaker notes, each sheet as a table, and PDF text page by page.
- Long files come back cut to `max_chars` with a hint such as `pages="4-9"`. Follow it to read the next part, or ask for the pages, slides or sheets you need.
- Big sheets show up to 500 rows and 40 columns at a time. Read the rest with `rows` (such as `rows="501-1000"`) and `columns` (such as `columns="AO-BZ"`), as the hint under the table says.
- Never write or run a script (Python, PowerShell, Node, unzip and the like) to read these files, and do not open them with `read_file`.
- When `read_document` reports a problem (encrypted, damaged, too large, or a scanned PDF without text), tell the user what it said instead of working around it.
