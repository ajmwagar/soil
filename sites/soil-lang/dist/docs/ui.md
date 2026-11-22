# UI forms

> Forms, escaping, and HTML rendering for the four-function UI contract.

> Generated from Soil doc comments. Do not edit by hand.

## `el`

```soil
(defun el (tag attrs children))
```

Build a UI form from a tag, attribute list, and child list.

Attributes are `(attr key value)` pairs. Children may be forms, strings, lists of forms, or `nil`.

## `attr`

```soil
(defun attr (key value))
```

Build one attribute pair for `el`.

## `txt`

```soil
(defun txt (value))
```

Wrap a value as an escaped text node.

## `raw`

```soil
(defun raw (html))
```

Wrap pre-rendered HTML without escaping.

This is an explicit escape hatch. Never pass untrusted input.

## `html-escape`

```soil
(defun html-escape (text))
```

Escape `&`, `<`, and `>` for HTML text.

## `attr-escape`

```soil
(defun attr-escape (value))
```

Escape a value for a double-quoted HTML attribute.

## `attr-name`

```soil
(defun attr-name (key))
```

Strip the leading colon from a keyword attribute name.

## `render-attr`

```soil
(defun render-attr (pair))
```

Render one attribute pair as HTML, or an empty string for `nil`/false.

## `render-attrs`

```soil
(defun render-attrs (attrs))
```

Render an attribute list as a leading space plus HTML attributes.

## `void-element?`

```soil
(defun void-element? (tag))
```

Return true for HTML elements that must not have a closing tag.

## `render-tag`

```soil
(defun render-tag (form))
```

Render one tag form, including attributes and children.

## `render-html`

```soil
(defun render-html (form))
```

Render a UI form, a list of forms, or a text node as HTML.

## `render-page`

```soil
(defun render-page (title styles body))
```

Wrap rendered HTML in a complete document with `title`, `styles`, and `body`.

