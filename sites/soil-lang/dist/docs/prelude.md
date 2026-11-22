# Prelude

> Lists, records, JSON, and the small helpers every Soil program uses.

> Generated from Soil doc comments. Do not edit by hand.

## `map`

```soil
(defun map (f xs))
```

Apply `f` to every element of `xs` and return a new list.

## `filter`

```soil
(defun filter (p xs))
```

Return the elements of `xs` for which `p` is true.

## `any?`

```soil
(defun any? (p xs))
```

Return true when `p` is true for at least one element of `xs`.

## `all?`

```soil
(defun all? (p xs))
```

Return true when `p` is true for every element of `xs`.

## `find`

```soil
(defun find (p xs))
```

Return the first element of `xs` for which `p` is true, or `nil`.

## `take`

```soil
(defun take (n xs))
```

Return the first `n` elements of `xs`.

## `drop`

```soil
(defun drop (n xs))
```

Return `xs` without its first `n` elements.

## `append`

```soil
(defun append (xs ys))
```

Concatenate two lists.

## `reverse`

```soil
(defun reverse (xs))
```

Return `xs` in reverse order.

## `nth`

```soil
(defun nth (n xs))
```

Return the zero-based element `n` of `xs`, or `nil` when out of range.

## `list-set`

```soil
(defun list-set (n value xs))
```

Return a copy of `xs` with zero-based element `n` replaced by `value`.

## `range`

```soil
(defun range (start end))
```

Return the integers from `start` (inclusive) to `end` (exclusive).

## `map-with`

```soil
(defun map-with (f arg xs))
```

Like `map`, but passes the extra argument `arg` to `f`.

## `filter-with`

```soil
(defun filter-with (p arg xs))
```

Like `filter`, but passes the extra argument `arg` to `p`.

## `take-while`

```soil
(defun take-while (pred xs))
```

Return the prefix of `xs` for which `pred` stays true.

## `drop-while`

```soil
(defun drop-while (pred xs))
```

Return `xs` after the prefix for which `pred` stays true.

## `last-of`

```soil
(defun last-of (xs))
```

Return the final element of a non-empty list.

## `reduce`

```soil
(defun reduce (f acc xs))
```

Fold `xs` from left to right, starting at `acc`.

## `assoc`

```soil
(defun assoc (key value record))
```

Return `record` with `key` bound to `value`; other pairs are preserved.

## `json-get`

```soil
(defun json-get (value key))
```

Read `key` from a decoded JSON object.

## `json-get-in`

```soil
(defun json-get-in (value keys))
```

Read a path of keys from a decoded JSON object.

## `page-entries`

```soil
(defun page-entries (page))
```

Return the `:entries` list of a records page.

## `entry-key`

```soil
(defun entry-key (entry))
```

Return the `:key` record from a records entry.

## `entry-id`

```soil
(defun entry-id (entry))
```

Return the id part of a records entry key.

## `entry-collection`

```soil
(defun entry-collection (entry))
```

Return the collection part of a records entry key.

## `entry-schema`

```soil
(defun entry-schema (entry))
```

Return the schema name of a records entry.

## `entry-updated-at`

```soil
(defun entry-updated-at (entry))
```

Return the update timestamp of a records entry, in milliseconds.

## `entry-body`

```soil
(defun entry-body (entry))
```

Decode and return the JSON body of a records entry.

## `record-key`

```soil
(defun record-key (collection id))
```

Build a records key from a collection name and an id.

## `make-entry`

```soil
(defun make-entry (collection id schema body now))
```

Build a records entry from a key, schema, body, and timestamp.

## `upsert-record`

```soil
(defun upsert-record (collection id schema body now))
```

Write an entry through the host records capability.

## `now-ms`

```soil
(defun now-ms ())
```

Return the host wall clock as Unix milliseconds.

## `caar`

```soil
(defun caar (x))
```

Return `(car (car x))`.

## `cadr`

```soil
(defun cadr (x))
```

Return `(car (cdr x))`.

## `cdar`

```soil
(defun cdar (x))
```

Return `(cdr (car x))`.

## `caddr`

```soil
(defun caddr (x))
```

Return `(car (cdr (cdr x)))`.

## `cadar`

```soil
(defun cadar (x))
```

Return `(car (cdr (car x)))`.

## `caddar`

```soil
(defun caddar (x))
```

Return `(car (cdr (cdr (car x))))`.

