# Purchase terms: status and checkout handoff

Prepared on 28 October 2026. The source repository is public again; the payment
link remains deactivated. The German and English terms are reviewable drafts, not a
claim of legal clearance. No customer has been messaged and no new contractual
terms have been applied retrospectively.

## Decisions reflected in the draft

- Keep the existing EUR 4.99 once-per-human licence for all 1.x and the existing
  free-use thresholds. Keep the BSL text and third-party licences unchanged.
- Cover consumers as well as businesses. The Code Translator's old B2B-only offer
  cannot be copied into this product without changing its intended offer.
- Describe a self-hosted transport product, including its optional model-process
  supervisor. Do not describe it as having no effect except moving chat bytes:
  it can deliberately start and stop an allowlisted model process.
- Make model/agent/tool output and operator-granted execution permissions separate
  from the supplier's obligations. Preserve liability for our own defects.
- Require IT notification and the necessary approval before use on managed networks.
  Do not promise successful connectivity through every NAT, firewall or proxy.
- Use a qualified liability clause for ordinary negligence, with essential duties
  and mandatory exceptions preserved. Do not promise that every claim is capped
  at the licence price; a refund and damages are different remedies.
- No inference-runtime acceptance dialog, DRM, online activation or payment callback.
  Purchase terms belong in checkout. Mere program use is not blanket risk consent.
- Preserve consumer withdrawal rights in the proposed checkout; no immediate-delivery
  waiver is requested in this draft. No new voluntary blanket refund promise for
  business buyers is added.

## Before reopening checkout

1. Have the contract model, liability clause and German/English wording reviewed by
   a qualified lawyer. Verify supplier details and required pre-contract information,
   including telephone/contact, tax and dispute-resolution disclosures as applicable.
   The postal address was taken from the existing Code Translator privacy notice;
   the live imprint could not be retrieved during this check.
2. Host the final terms, withdrawal notice, product-specific privacy notice and
   imprint at public, stable, versioned addresses. A private GitHub link is not
   accessible notice to a purchaser. The old repository's sales terms and release
   assets are preserved privately for existing purchase records.
3. In the product's checkout, show system requirements, network limitations, the
   licence scope and final total. Require an affirmative, non-preselected acceptance
   of the applicable purchase terms before payment. Preserve the accepted version
   with the order and provide contract confirmation and terms on a durable medium.
4. Implement and test the consumer online withdrawal function on the relevant
   purchase interface, including submission and prompt durable receipt. An email
   address in a Markdown file is not this function. Keep ordinary withdrawal
   methods available as well. Do not confuse withdrawal with subscription cancellation.
5. Check Stripe's actual configuration for this product. Managed Payments is disabled
   here; do not copy the other product's assertion that Link is merchant of record.
   Avoid changing an account-wide legal URL so that unrelated products suddenly use
   these terms. Verify customer-visible links, consent and confirmation end to end.
6. Reopen sales only when the actual delivery and legal checkout flow are ready.
   Distinguish portable Windows/macOS transport previews from the Linux/NVIDIA
   Docker inference deployment. State signature status and the actual test scope;
   a native build does not provide application network isolation or a safety certification.

## Sources checked

- [BGB § 305: incorporation of standard terms](https://www.gesetze-im-internet.de/bgb/__305.html)
- [BGB § 307: unfair standard terms](https://www.gesetze-im-internet.de/bgb/__307.html)
- [BGB § 309: prohibited limitations and burden-of-proof clauses](https://www.gesetze-im-internet.de/bgb/__309.html)
- [BGB § 327: consumer digital products](https://www.gesetze-im-internet.de/bgb/__327.html)
- [BGB § 327f: required updates](https://www.gesetze-im-internet.de/bgb/__327f.html)
- [BGB § 355: consumer withdrawal](https://www.gesetze-im-internet.de/bgb/__355.html)
- [BGB § 356: digital-content withdrawal conditions, currently paragraph 6](https://www.gesetze-im-internet.de/bgb/__356.html)
- [BGB § 356a: electronic withdrawal function](https://www.gesetze-im-internet.de/bgb/__356a.html)
- [BGB § 312f: contract confirmation](https://www.gesetze-im-internet.de/bgb/__312f.html)
- [EGBGB Art. 246a § 1: pre-contract information](https://www.gesetze-im-internet.de/bgbeg/art_246a__1.html)

The existing Code Translator terms supplied a useful scope/liability structure,
but its subscription, B2B-only restriction, licence type and checkout roles were
not transferred to k-lani-link.
