# Private business workflow demonstration

Everything in this folder is synthetic and may appear in a public demonstration.
The task resembles a workflow involving customer contact and financial data;
no real personal data, tax identifiers, account numbers or payment instructions
are present. Use English for the entire task.

Create the outputs yourself with the local Basalt model. The operator supplies
the brief and watches the run. There is no second model provider. Do not access
the internet, install packages, spawn subagents, commit, push, or publish.
Only change files under `/work/demo/output`. Preserve the input files and logo.

## Phase 1: invoice and spreadsheet

Read `customer.json`. Use the **WordCraft MCP tools** to create a clearly laid out
one-page invoice in `output/invoice.docx` and `output/invoice.pdf`. Include the
fictional issuer and customer, invoice dates/number, all line items, calculated
net/tax/gross amounts, and the prominent fictional-data notice. The 20% rate is
an illustrative input, not tax advice. No bank details or request for payment.
Read the document back with WordCraft to verify content before saving both formats.

Use the **GridCraft MCP tools** to create `output/ledger.xlsx`. Include the customer
contact and invoice metadata, line items with quantity × rate formulas, and net,
tax and gross totals calculated by spreadsheet formulas. Make the sheet readable
with headers, appropriate widths and currency/percentage formats. Read back the
calculated cells to verify them, then save the workbook. Also save a local HTML
preview as `output/ledger.html` through GridCraft.

Use the tool catalogs for command names and parameter schemas; do not guess them.
Do not replace WordCraft/GridCraft with a Python DOCX/PDF/XLSX implementation.
If something fails, diagnose and correct it using the same local agent and tools.
Report filenames, totals and any remaining limitations in English.

## Phase 2: website, when the operator requests it

Build `output/site/index.html` and optional local CSS/JS. This is a small English
landing page about **k-lani-link**. Use the original supplied image at
`assets/k-lani-link.webp` (copy it into the site). Use no external assets,
packages, fonts, CDNs, telemetry or browser network calls.

Explain the actual architecture: a client app such as OpenCode uses a local
OpenAI-compatible API; k-lani-link authenticates the two peers and carries their
requests over WebRTC with DTLS and additional content encryption. The signaling
connection closes after pairing. TURN is disabled by default and requires an
explicit setting. Responses, reasoning fields and tool calls can stream when
the model/provider and client support them. The inference backend is replaceable.

Show this invoice-and-ledger use case without claiming automatic GDPR compliance
or certification. Explain that organizational policy, device security and the
agent's local history still matter. In this recorded lab both peers are on one
machine; it is not a remote office-to-home benchmark. Do not invent speeds or
claim the whole 256K window was used. No claim that every browser/extension or
client is already supported. Make it responsive, readable and visually polished.
Validate internal links and local resources. Do not change the invoice or ledger.
