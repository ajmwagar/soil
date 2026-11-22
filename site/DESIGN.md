# Site design brief

## Audience

Developers and agent builders deciding whether a new language is worth their
time. They already know what a runtime is. They do not need another framework
pitch.

## Argument

1. WebAssembly is fast, portable, and already where the work runs.
2. In 2026, agents write most of the code and humans review less of it. The
   source has to be legible to both.
3. Soil is homoiconic: code, data, macros, and tests are one tree, so an agent
   does not translate the program into a second shape.
4. That idea is old. Paul Graham's Viaweb used Lisp macros to ship faster.
   Looking backwards is sometimes how you go forwards.

## Structure

Specimen sheet → why another language → core/std index → Loam runtime →
real programs → closing. Do not fall back to hero → three features → CTA.

## Voice

Dry, specific, and short. No invented metrics, testimonials, logos, or proof
bars. No “10× faster”, “trusted by”, or fabricated case studies. State what the
code does.

## Theme

Warm charcoal, rust accent, serif display type, system sans body, mono code.
Every colour and font family is a named token.

## Hard rules

- No fake browser bars, phone frames, code-window chrome, or IDE chrome.
- No section kickers on the landing page. The guide may have one.
- No horizontal scroll at 320 / 375 / 414 / 768 px.
- Grids use `minmax(0, 1fr)`.
- `overflow-x: clip` on `html` and `body`.
- Display headings wrap with `overflow-wrap: anywhere`.
- Links, buttons, and nav items stay on one line.
- Examples must use the same warm-charcoal tokens as the site.

## Sponsor

The footer names [Future Present Labs](https://fpl.dev/) as sponsor.
