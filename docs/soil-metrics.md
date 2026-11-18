# `soil:metrics` — a Spore's numbers, the host's endpoint

Status: **wired into both hosts; `/metrics` served by the gateway.** The
`runner` world imports `soil:metrics/metrics` and `HttpHost` implements `emit`
and `snapshot` over a registry; a manifest-hosted Spore gets the same interface
on `WasiHost`, gated on the `[grants]` entry. The gateway answers `GET /metrics`
next to `/healthz` and `/readyz`.

## Why the host owns the registry

A metric is a fact about work the Spore already did, not an action: it must
never be able to fail the work, and it must never need a socket. So a Spore
names a series and a value, and the host keeps the registry:

- **One registry per process.** A host that runs several Spores merges their
  series. A series with two kinds (counter and gauge under one name) is refused
  rather than rendered twice.
- **The HTTP surface is the host's.** `GET /metrics` answers Prometheus text
  from the registry the host already holds. A Spore cannot choose a port, a
  path, a scraper, or a label that the host would not put on the wire.
- **A registry per Spore, one endpoint per vhost.** `HttpService` owns the
  registry its guest writes and renders it on that Spore's vhost, so a host
  running several Spores cannot merge one Spore's series into another's.
- **Budgets fail closed.** A name outside `[A-Za-z_][A-Za-z0-9_]*` (100 chars),
  a label value over 200 chars or containing a quote or newline, a non-finite
  value, more than 16 labels, or more than 10,000 series is refused at write
  time. A Spore that spams metrics loses the metric, not the run.

## The interface

```wit
enum kind { counter, gauge }
record sample {
  name: string, kind: kind, value: f64,
  help: option<string>, labels: list<tuple<string, string>>,
}
emit: func(samples: list<sample>) -> result<_, error>;
snapshot: func() -> list<sample>;
```

Counters add; gauges replace. Labels are sorted by the host, so two calls that
differ only in label order are one series.

## What gets rendered

```
# HELP soil_agents Agents by phase
# TYPE soil_agents gauge
soil_agents{host="batty",phase="working"} 3
```

`HELP` and `TYPE` are written once per name, before its series; label values
are escaped. That is the whole format a scraper needs.

## Alerting stays on the bus

A scrape is a sample; an alert is a decision with a person on the other end.
They travel separately: Prometheus scrapes `/metrics`, an alert rule fires, and
the *notification* is a `soil:unibus` frame — today the `fpl.node.health.v1`
shape that the fleet feed already consumes, and a `soil.metrics.alert.v1` kind
if the rules want their own vocabulary. A dashboard and an alert can then
disagree about a threshold, never about the number.

## Fleet-wide shape

Every host that serves a Spore surface exposes `/metrics` on the same listener
as the surface:

```text
Spore ── soil:metrics ──▶ host registry ──▶ GET /metrics ──▶ Prometheus
                                                    │
                                                    └─ alert rule ──▶ soil:unibus ──▶ fleet feed
```

That is what makes the fleet feed cheap to extend: a new alert is a rule and a
frame, not another shell script per host.

## What is wired

1. The `runner` world imports `soil:metrics/metrics`; `HttpHost` implements the
   generated host trait over [`Metrics`](../crates/soil/src/metrics.rs) and
   links it into `HttpRuntime`.
2. A manifest-hosted Spore gets the same interface on the `wasi:http` surface:
   `WasiHostBuilder` holds the registry, `HttpService` links the import and
   renders that registry, and `soil spore run` sets the grant from
   `[grants] "soil:metrics" = "..."` in `spore.toml`.
3. `emit` is `denied` without the grant. The import still links, so the Spore
   starts, discovers the missing grant on its first write, and its `/metrics`
   stays empty rather than the process failing to instantiate.
4. `GET /metrics` answers on every vhost, before the grant table, rendering
   `HttpService::metrics_text()`.
5. `Readiness` (`/readyz`) reports the bus link and the geode socket.

## Remaining

1. Move `fpl-node-health`'s checks (mesh, probes, connector, shroud, bob) into a
   Spore that emits them as metrics, so the shell script retires.
2. A `soil-sdk-metrics` guest crate (typed `Counter`/`Gauge` over the WIT) so a
   Spore does not hand-write the `sample` record.
