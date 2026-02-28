# OMI-Lite Protocol Specification

**Version 1.0 Draft**

---

## 1. Introduction

OMI-Lite is a lightweight IoT messaging and data protocol for constrained embedded devices. It unifies the concepts of O-MI v2.0 (Open Messaging Interface) and O-DF v2.0 (Open Data Format) — both Open Group Standards — into a single, minimal specification that uses JSON instead of XML.

### 1.1 Goals

1. **Minimal footprint**: implementable on devices with 320 KB SRAM and limited CPU
2. **Low parsing overhead**: JSON instead of XML, flat key-value structures where possible
3. **Unified spec**: one protocol for both messaging and data (no separate O-MI + O-DF)
4. **Preserve the good parts**: hierarchical object tree, path-based addressing, subscriptions, history queries, REST discovery
5. **Drop the bloat**: SOAP/WSDL, nodeList routing, complex identity types, XML namespaces, altnames, and rarely-used features
6. **Practical P2P**: designed for device-to-device communication on local networks

### 1.2 Design Rationale

The O-MI/O-DF standards provide a well-designed conceptual model for IoT data and messaging. However, practical implementation on constrained devices (as demonstrated by the eOMI ESP32-S2 implementation) reveals significant overhead:

- XML parsing requires streaming parsers with custom state machines to avoid DOM construction
- XML character escaping (`&lt;`, `&gt;`) complicates embedded script storage
- Separate O-MI and O-DF specs create redundant envelope nesting
- Features like nodeList routing, call operation metadata, IoTIdType date ranges, and altnames add complexity without practical benefit for P2P use cases
- SOAP/WSDL bindings are dead weight for HTTP-native devices

OMI-Lite addresses these by using JSON (with optional CBOR for binary efficiency), merging messaging and data into one spec, and cutting features that constrained devices don't need.

---

## 2. Transport Layer

### 2.1 HTTP

All OMI-Lite messages are sent as **HTTP POST** to the endpoint `/omi`.

```
POST /omi HTTP/1.1
Content-Type: application/json
```

Responses use standard HTTP status codes for transport-level errors (e.g., 400 for malformed JSON). Application-level status is in the response body.

### 2.2 REST Discovery (HTTP GET)

HTTP GET on `/omi/` and sub-paths provides REST discovery of the object tree.

| URL | Returns |
|-----|---------|
| `GET /omi/` | Top-level objects (id, type, description) |
| `GET /omi/{ObjectId}/` | Object details: child objects and InfoItems |
| `GET /omi/{ObjectId}/{InfoItem}` | Latest value(s) of the InfoItem |
| `GET /omi/{Obj}/{SubObj}/{Item}` | Nested path traversal to any depth |

**Query parameters** for GET requests:

| Parameter | Type | Description |
|-----------|------|-------------|
| `newest` | integer | Return N newest values |
| `oldest` | integer | Return N oldest values |
| `begin` | number | Unix timestamp. Values from this time. |
| `end` | number | Unix timestamp. Values until this time. |
| `depth` | integer | Max child levels to return (0 = this node only) |

GET responses use the same JSON data model as POST responses (Section 4).

A trailing slash distinguishes object listings from InfoItem value retrieval:
- `/omi/DeviceA/` — returns object `DeviceA` with its children
- `/omi/DeviceA/Temperature` — returns InfoItem `Temperature` with its value(s)

### 2.3 WebSocket

A WebSocket endpoint at `/omi/ws` provides persistent bidirectional communication, primarily for subscription delivery.

- Clients connect via standard WebSocket upgrade at `/omi/ws`
- Once connected, clients may send OMI-Lite JSON messages (same format as HTTP POST)
- The server delivers subscription updates over the WebSocket connection
- Multiple subscriptions can share one WebSocket connection

---

## 3. Content Negotiation

The default encoding is JSON. CBOR is supported as a compact binary alternative.

| Content-Type | Description |
|--------------|-------------|
| `application/json` | Default. Human-readable JSON encoding. |
| `application/cbor` | Binary CBOR encoding. Same data model, compact wire format. |

Clients indicate their preferred format via the `Content-Type` header (for requests) and `Accept` header (for responses). If no `Accept` header is provided, the response uses the same format as the request.

For WebSocket connections, the encoding is negotiated once during the initial handshake via the `Sec-WebSocket-Protocol` header with values `omi-json` (default) or `omi-cbor`.

---

## 4. Data Model

OMI-Lite uses a hierarchical object tree, addressed by slash-separated paths. This unifies the O-DF Object/InfoItem/value hierarchy into a single JSON structure.

### 4.1 Tree Structure

```
/                          Root (contains top-level objects)
/DeviceA/                  Object
/DeviceA/SubDevice/        Nested Object
/DeviceA/Temperature       InfoItem (leaf — holds values)
```

### 4.2 Object

An Object is a named container that holds InfoItems and/or child Objects.

```json
{
  "id": "DeviceA",
  "type": "https://schema.org/Thing",
  "desc": "My sensor device",
  "items": {
    "Temperature": { ... },
    "Humidity": { ... }
  },
  "objects": {
    "SubDevice": { ... }
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Unique identifier. Used as the path segment. |
| `type` | string | no | Semantic type URI (e.g., `https://schema.org/Place`). |
| `desc` | string | no | Human-readable description. |
| `items` | object | no | Map of InfoItem name → InfoItem. |
| `objects` | object | no | Map of Object id → Object (recursive nesting). |

### 4.3 InfoItem

An InfoItem represents a named property, sensor reading, actuator state, or method. It holds timestamped values.

```json
{
  "type": "https://schema.org/Float",
  "desc": "Outdoor temperature in Celsius",
  "meta": {
    "unit": "Celsius",
    "accuracy": 0.1,
    "writable": true
  },
  "values": [
    {"v": 21.5, "t": 1709136000},
    {"v": 21.3, "t": 1709135700}
  ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | string | no | Semantic type URI. |
| `desc` | string | no | Human-readable description. |
| `meta` | object | no | Key-value metadata (unit, accuracy, writable, etc.). |
| `values` | array | no | Array of timestamped values, newest first. |

### 4.4 Value

A timestamped data point.

```json
{"v": 21.5, "t": 1709136000}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `v` | any | yes | The value. Can be number, string, boolean, object, or array. |
| `t` | number | no | Unix timestamp (seconds since epoch, fractional allowed). Auto-generated by the receiver if omitted. |

### 4.5 Metadata

Metadata is a flat key-value object attached to an InfoItem. Values can be any JSON type.

```json
{
  "unit": "Celsius",
  "accuracy": 0.1,
  "readable": true,
  "writable": false,
  "format": "double",
  "latency": 10
}
```

Unlike O-DF where MetaData contains nested InfoItem elements (each with their own value/type structure), OMI-Lite metadata is flat key-value pairs. This eliminates an entire level of nesting and parsing.

Common metadata keys (all optional):

| Key | Type | Description |
|-----|------|-------------|
| `unit` | string | Measurement unit (SI preferred). |
| `accuracy` | number | Measurement accuracy. |
| `format` | string | Value encoding (e.g., `"double"`, `"int"`, `"bool"`). |
| `readable` | boolean | Whether the value can be read. Default: true. |
| `writable` | boolean | Whether the value can be written. Default: false. |
| `latency` | number | Expected update latency in seconds. |

Implementations may define additional keys as needed.

---

## 5. Message Envelope

Every OMI-Lite message is a JSON object with the following structure:

```json
{
  "omi": "1.0",
  "ttl": 10,
  "<operation>": { ... }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `omi` | string | yes | Protocol version. Must be `"1.0"`. |
| `ttl` | number | yes | Time-to-live in seconds. `0` = respond immediately. `-1` = no expiry. |
| `<operation>` | object | yes | Exactly one of: `read`, `write`, `delete`, `cancel`, `response`. |

The envelope contains exactly **one** operation per message. This matches the O-MI design where each omiEnvelope contains one operation.

---

## 6. Operations

### 6.1 Read

Retrieves data from the object tree. Also used to create subscriptions (when `interval` is present).

#### One-time Read

```json
{
  "omi": "1.0",
  "ttl": 10,
  "read": {
    "path": "/DeviceA/Temperature",
    "newest": 5
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path` | string | yes (unless `rid`) | Slash-separated path to read. `/` for root. |
| `rid` | string | no | Poll a subscription by request ID (see Section 7.5). Mutually exclusive with `path`. |
| `newest` | integer | no | Return N newest values. |
| `oldest` | integer | no | Return N oldest values. |
| `begin` | number | no | Unix timestamp. Return values from this time. |
| `end` | number | no | Unix timestamp. Return values until this time. |
| `depth` | integer | no | Max child levels to include. 0 = target node only. |

A read must contain either `path` (for data retrieval or subscription) or `rid` (for polling), but not both.

When `newest`/`oldest` is combined with `begin`/`end`, the time range is applied first, then the count limit.

#### Subscription Read

Adding `interval` turns a read into a subscription. The server returns a `rid` (request ID) for managing the subscription.

```json
{
  "omi": "1.0",
  "ttl": 3600,
  "read": {
    "path": "/DeviceA/Temperature",
    "interval": -1,
    "callback": "http://192.168.1.50/omi"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `interval` | number | yes (for subscription) | Subscription interval in seconds. Special values below. |
| `callback` | string | no | URL for delivery. Omit for WebSocket delivery. |

**Interval semantics:**

| Value | Meaning |
|-------|---------|
| `> 0` | Deliver at this interval in seconds. Delivers even if value unchanged. |
| `-1` | Deliver on every value change (event-based). |

If `callback` is omitted and the client has an active WebSocket connection, subscription updates are delivered over that WebSocket. If `callback` is provided, the server POSTs updates to that URL.

The `ttl` on a subscription message defines the subscription lifetime. After expiry, the server stops delivering updates.

### 6.2 Write

Writes values to the object tree. Can create new Objects/InfoItems if they don't exist.

The write object uses one of three mutually exclusive forms:

| Form | Required fields | Description |
|------|----------------|-------------|
| Single value | `path` + `v` | Write one value to one path |
| Batch | `items` | Write multiple values to multiple paths |
| Object tree | `path` + `objects` | Create/update object structure |

#### Write a Single Value

```json
{
  "omi": "1.0",
  "ttl": 10,
  "write": {
    "path": "/DeviceA/Temperature",
    "v": 21.5
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `path` | string | Target path. Required for single-value and object-tree forms. |
| `v` | any | Value to write. Timestamp auto-generated. |
| `t` | number | Explicit Unix timestamp for the value. |
| `items` | array | Batch write. Array of `{path, v, t?}` objects. Mutually exclusive with `v` and `objects`. |
| `objects` | object | Object tree to create/update. Map of id → Object. Mutually exclusive with `v` and `items`. |

#### Write a Value with Timestamp

```json
{
  "omi": "1.0",
  "ttl": 10,
  "write": {
    "path": "/DeviceA/Temperature",
    "v": 21.5,
    "t": 1709136000
  }
}
```

#### Write Multiple Values (Batch)

```json
{
  "omi": "1.0",
  "ttl": 10,
  "write": {
    "items": [
      {"path": "/DeviceA/Temperature", "v": 21.5},
      {"path": "/DeviceA/Humidity", "v": 55.2},
      {"path": "/DeviceB/Status", "v": "online"}
    ]
  }
}
```

When `items` is present, each entry is an object with `path`, `v`, and optional `t`.

#### Create an Object/InfoItem Structure

Writing to a path that doesn't exist creates it. Writing an Object tree creates the full structure:

```json
{
  "omi": "1.0",
  "ttl": 10,
  "write": {
    "path": "/",
    "objects": {
      "DeviceA": {
        "id": "DeviceA",
        "type": "https://schema.org/Thing",
        "desc": "My sensor device",
        "items": {
          "Temperature": {
            "type": "https://schema.org/Float",
            "desc": "Temperature in Celsius",
            "meta": {"unit": "Celsius", "writable": false},
            "values": [{"v": 21.5}]
          }
        }
      }
    }
  }
}
```

#### Write Behavior

- If the target path exists, its values are appended (not replaced).
- If the target path does not exist, it is created along with any missing parents.
- If subscriptions exist on the written path, they fire after the write completes.
- The server must not respond with success until the write is durable.

### 6.3 Delete

Removes an Object or InfoItem from the tree.

```json
{
  "omi": "1.0",
  "ttl": 10,
  "delete": {
    "path": "/DeviceA/Temperature"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path` | string | yes | Path to delete. Deletes the target and all children. |

- Deleting an Object deletes all its InfoItems and child Objects.
- Deleting an InfoItem deletes all its values, metadata, and description.
- Active subscriptions on deleted paths receive an empty value notification, then stop.
- Deleting `/` is forbidden (returns error 403).

### 6.4 Cancel

Cancels one or more active subscriptions by their request IDs.

```json
{
  "omi": "1.0",
  "ttl": 10,
  "cancel": {
    "rid": ["abc123", "def456"]
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `rid` | array of strings | yes | Request IDs of subscriptions to cancel. |

### 6.5 Response

Sent in reply to any operation. Also used for subscription delivery.

```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 200,
    "rid": "abc123",
    "desc": "OK",
    "result": { ... }
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `status` | integer | yes | HTTP-style status code (200, 400, 404, 500, etc.). |
| `rid` | string | no | Request ID. Present for subscription responses and subscription setup. |
| `desc` | string | no | Human-readable status description. |
| `result` | any | no | The result data. Structure depends on the original operation (see below). |

**Result structure by operation:**

| Original operation | `result` structure |
|---|---|
| `read` (one-time) | Object or InfoItem at the requested path. Includes `path` and `values` for InfoItems, or full Object structure for objects. |
| `read` (subscription setup) | Not present. Only `rid` is returned. |
| `read` (subscription delivery) | `{"path": "...", "values": [...]}` — the updated path and its new values. |
| `read` (poll) | `{"path": "...", "values": [...]}` — accumulated values since last poll. |
| `write` | Not present on success. For batch writes with partial failure, an array of `{"path", "status", "desc?"}` per item. |
| `delete` | Not present. |
| `cancel` | Not present. |

**Status codes:**

| Code | Meaning |
|------|---------|
| 200 | Success |
| 201 | Created (new Object/InfoItem written) |
| 400 | Bad request (malformed message) |
| 403 | Forbidden (e.g., write to read-only InfoItem) |
| 404 | Not found (path does not exist) |
| 408 | Request timeout (TTL expired) |
| 500 | Internal server error |
| 501 | Not implemented (unsupported operation) |

---

## 7. Subscription Mechanics

### 7.1 Creating a Subscription

Send a `read` with `interval` set. The server immediately responds with a `rid`:

**Request:**
```json
{
  "omi": "1.0",
  "ttl": 3600,
  "read": {
    "path": "/DeviceA/Temperature",
    "interval": -1,
    "callback": "http://192.168.1.50/omi"
  }
}
```

**Immediate response (does not include data):**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 200,
    "rid": "sub-001",
    "desc": "Subscription created"
  }
}
```

### 7.2 Subscription Delivery

When a subscribed value changes (event-based) or the interval elapses (interval-based), the server sends a response to the callback URL or over the WebSocket:

```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 200,
    "rid": "sub-001",
    "result": {
      "path": "/DeviceA/Temperature",
      "values": [{"v": 22.1, "t": 1709137200}]
    }
  }
}
```

### 7.3 WebSocket Subscriptions

When a subscription is created without a `callback` and the client has an active WebSocket connection, updates are delivered over that WebSocket. This is the preferred mechanism for constrained devices because:

- No need for the subscriber to run an HTTP server
- NAT traversal is handled by the persistent connection
- Lower overhead than repeated HTTP requests

### 7.4 Subscription Lifetime

A subscription lives until:
1. The `ttl` expires (counted from subscription creation), or
2. A `cancel` message is received with the subscription's `rid`, or
3. The WebSocket connection closes (for WebSocket-delivered subscriptions without a callback)

### 7.5 Polling (Callback-less HTTP)

If a subscription is created over HTTP POST without a `callback`, the server stores updates for later retrieval. The client polls by sending a read with the `rid`:

```json
{
  "omi": "1.0",
  "ttl": 10,
  "read": {
    "rid": "sub-001"
  }
}
```

This returns all accumulated values since the last poll.

---

## 8. Error Handling

### 8.1 Transport Errors

Standard HTTP status codes are used at the transport level (e.g., 400 for unparseable JSON, 405 for wrong HTTP method).

### 8.2 Application Errors

Application-level errors are returned in the response body:

```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 404,
    "desc": "Path not found: /DeviceA/Nonexistent"
  }
}
```

### 8.3 TTL Expiry

If a server cannot fulfill a request before its TTL expires, it returns status 408:

```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 408,
    "desc": "Request timed out"
  }
}
```

### 8.4 Partial Success (Batch Write)

When a batch write partially succeeds, the response includes per-item results:

```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 200,
    "desc": "Partial success",
    "result": [
      {"path": "/DeviceA/Temperature", "status": 200},
      {"path": "/DeviceA/ReadOnly", "status": 403, "desc": "Not writable"}
    ]
  }
}
```

---

## 9. Complete Examples

### 9.1 Discover the Object Tree

**Request:**
```
GET /omi/ HTTP/1.1
Accept: application/json
```

**Response:**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 200,
    "result": {
      "objects": {
        "Dht11": {
          "id": "Dht11",
          "type": "https://schema.org/Thing",
          "desc": "DHT11 humidity and temperature sensor"
        },
        "Servo": {
          "id": "Servo",
          "desc": "Servo motor controller"
        }
      }
    }
  }
}
```

### 9.2 Read an Object's Children

**Request:**
```
GET /omi/Dht11/ HTTP/1.1
Accept: application/json
```

**Response:**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 200,
    "result": {
      "id": "Dht11",
      "type": "https://schema.org/Thing",
      "items": {
        "RelativeHumidity": {
          "type": "https://schema.org/Float",
          "desc": "Relative humidity percentage",
          "meta": {"unit": "%RH", "accuracy": 5.0},
          "values": [{"v": 45.2, "t": 1709136000}]
        },
        "Temperature": {
          "type": "https://schema.org/Float",
          "meta": {"unit": "Celsius"},
          "values": [{"v": 22.1, "t": 1709136000}]
        }
      }
    }
  }
}
```

### 9.3 Read Latest Value

**Request:**
```
GET /omi/Dht11/RelativeHumidity HTTP/1.1
```

**Response:**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 200,
    "result": {
      "values": [{"v": 45.2, "t": 1709136000}]
    }
  }
}
```

### 9.4 Read History

**Request:**
```json
{
  "omi": "1.0",
  "ttl": 10,
  "read": {
    "path": "/Dht11/RelativeHumidity",
    "begin": 1709130000,
    "end": 1709136000,
    "newest": 10
  }
}
```

**Response:**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 200,
    "result": {
      "path": "/Dht11/RelativeHumidity",
      "values": [
        {"v": 45.2, "t": 1709136000},
        {"v": 44.8, "t": 1709135400},
        {"v": 44.1, "t": 1709134800}
      ]
    }
  }
}
```

### 9.5 Write a Sensor Value

**Request:**
```json
{
  "omi": "1.0",
  "ttl": 10,
  "write": {
    "path": "/Dht11/RelativeHumidity",
    "v": 61.3
  }
}
```

**Response:**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {"status": 200}
}
```

### 9.6 Publish a New Device (Create Object Tree)

**Request:**
```json
{
  "omi": "1.0",
  "ttl": 10,
  "write": {
    "path": "/",
    "objects": {
      "AirConditioner": {
        "id": "AirConditioner",
        "type": "https://schema.org/Thing",
        "desc": "Living room AC unit",
        "items": {
          "Temperature": {
            "desc": "Current temperature",
            "meta": {"unit": "Celsius", "readable": true, "writable": false}
          },
          "SetPoint": {
            "desc": "Target temperature",
            "meta": {"unit": "Celsius", "readable": true, "writable": true},
            "values": [{"v": 24.0}]
          },
          "Switch": {
            "desc": "On/off state",
            "meta": {"writable": true},
            "values": [{"v": false}]
          }
        }
      }
    }
  }
}
```

**Response:**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {"status": 201, "desc": "Created"}
}
```

### 9.7 Event Subscription (On Change)

**Request:**
```json
{
  "omi": "1.0",
  "ttl": 3600,
  "read": {
    "path": "/Dht11/RelativeHumidity",
    "interval": -1,
    "callback": "http://192.168.1.50/omi"
  }
}
```

**Immediate response:**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 200,
    "rid": "sub-evt-001",
    "desc": "Subscription created"
  }
}
```

**Subsequent callback POST to `http://192.168.1.50/omi` when value changes:**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 200,
    "rid": "sub-evt-001",
    "result": {
      "path": "/Dht11/RelativeHumidity",
      "values": [{"v": 55.7, "t": 1709137500}]
    }
  }
}
```

### 9.8 Interval Subscription

**Request:**
```json
{
  "omi": "1.0",
  "ttl": 86400,
  "read": {
    "path": "/Dht11/Temperature",
    "interval": 60,
    "callback": "http://192.168.1.50/omi"
  }
}
```

Delivers the current value of `/Dht11/Temperature` every 60 seconds for 24 hours.

### 9.9 WebSocket Subscription

After establishing a WebSocket connection to `/omi/ws`:

**Client sends:**
```json
{
  "omi": "1.0",
  "ttl": 7200,
  "read": {
    "path": "/Dht11/RelativeHumidity",
    "interval": -1
  }
}
```

**Server responds (over WebSocket):**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {
    "status": 200,
    "rid": "sub-ws-001",
    "desc": "Subscription created"
  }
}
```

No `callback` — subsequent updates arrive on this WebSocket connection. The `rid` can be used to cancel the subscription later.

### 9.10 Cancel a Subscription

**Request:**
```json
{
  "omi": "1.0",
  "ttl": 10,
  "cancel": {
    "rid": ["sub-evt-001"]
  }
}
```

**Response:**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {"status": 200, "desc": "Cancelled 1 subscription(s)"}
}
```

### 9.11 Delete an InfoItem

**Request:**
```json
{
  "omi": "1.0",
  "ttl": 10,
  "delete": {
    "path": "/AirConditioner/SetPoint"
  }
}
```

**Response:**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {"status": 200}
}
```

### 9.12 Batch Write (Multiple Values)

**Request:**
```json
{
  "omi": "1.0",
  "ttl": 10,
  "write": {
    "items": [
      {"path": "/Dht11/RelativeHumidity", "v": 48.5},
      {"path": "/Dht11/Temperature", "v": 23.1},
      {"path": "/Servo/Switch", "v": true}
    ]
  }
}
```

**Response:**
```json
{
  "omi": "1.0",
  "ttl": 0,
  "response": {"status": 200}
}
```

### 9.13 Bathroom Fan Scenario (from eOMI thesis)

This demonstrates the complete use case: a humidity sensor on Device 1 controls a fan motor on Device 2 via event subscriptions and write-triggered scripts.

**Step 1: Device 2 publishes its structure**

Device 2 sends to its own local tree:
```json
{
  "omi": "1.0", "ttl": 10,
  "write": {
    "path": "/",
    "objects": {
      "Servo": {
        "id": "Servo",
        "items": {
          "Switch": {"meta": {"writable": true}, "values": [{"v": false}]},
          "SetDegrees": {"meta": {"writable": true, "unit": "degrees"}, "values": [{"v": 0}]}
        }
      }
    }
  }
}
```

**Step 2: Device 1 creates an event subscription on Device 2**

A configurator sends to Device 1 (`192.168.1.10`):
```json
{
  "omi": "1.0", "ttl": -1,
  "read": {
    "path": "/Dht11/RelativeHumidity",
    "interval": -1,
    "callback": "http://192.168.1.20/omi"
  }
}
```

Now every humidity change on Device 1 is sent to Device 2.

**Step 3: Device 2 has an onwrite script on its `/Dht11/RelativeHumidity` path**

The script (installed via write to metadata, implementation-defined):
```javascript
if (!global.hasOwnProperty("triggered")) global.triggered = false;
if (event.value > 46 && !global.triggered) {
  global.triggered = true;
  odf.writeItem(true, "/Servo/Switch");
}
if (event.value < 35 && global.triggered) {
  global.triggered = false;
  odf.writeItem(false, "/Servo/Switch");
}
```

When humidity exceeds 46%, the fan turns on. When it drops below 35%, the fan turns off. The `global` object maintains state across invocations, providing hysteresis.

---

## 10. Comparison: O-MI/O-DF v2.0 vs OMI-Lite

| Aspect | O-MI/O-DF v2.0 | OMI-Lite | Rationale |
|--------|----------------|----------|-----------|
| **Encoding** | XML | JSON (default), CBOR (optional) | JSON is smaller, faster to parse, no escaping issues for embedded scripts |
| **Specifications** | Two separate specs (O-MI + O-DF) | One unified spec | Eliminates redundant concepts and simplifies implementation |
| **Envelope** | `<omiEnvelope version="2.0" ttl="10">` | `{"omi":"1.0","ttl":10}` | Same semantics, less verbosity |
| **Data tree** | `<Objects><Object><InfoItem><value>` | `{objects:{id:{items:{name:{values:[]}}}}}` | Same hierarchy, JSON encoding |
| **Object identity** | `<id>` element (IoTIdType with idType, tagType, startDate, endDate) | `"id"` string field | IoTIdType complexity (tag types, date ranges) unused in practice |
| **InfoItem identity** | `name` attribute + `<altname>` elements | Key in parent's `items` map | altname adds complexity; use the canonical name |
| **Semantic typing** | `type` attribute + `prefix` for compact URIs + O-DEF scheme | `type` field with full URI | O-DEF prefix system rarely used; full URIs are simpler and unambiguous |
| **MetaData** | Nested `<InfoItem>` elements inside `<MetaData>` | Flat `meta` key-value object | Eliminates recursive nesting; MetaData is always simple key-value in practice |
| **Timestamps** | `dateTime` (ISO 8601) + `unixTime` (double) | `t` (Unix timestamp, double) | One format. Unix timestamps are compact and easy to compare |
| **Operations** | read, write, call, delete, cancel | read, write, delete, cancel | `call` dropped — use write + onwrite scripts (proven in eOMI thesis) |
| **Subscriptions** | `interval` on read: -2, -1, 0, >0 | `interval` on read: -1, >0 | Dropped -2 (on-connect) and 0 (fast-poll) — rarely needed, potential attack vector |
| **Routing** | `<nodeList>` with `<node>` URIs | Not supported | P2P devices talk directly; routing is a gateway concern |
| **Discovery** | O-DF HTTP GET + `maxlevels` | HTTP GET on `/omi/` paths + `depth` param | Same concept, cleaner REST paths |
| **Response** | `<response><result><return returnCode="200">` | `{"response":{"status":200}}` | Same HTTP status codes, flatter structure |
| **Synchronous dialog** | Nested `<omiEnvelope>` inside `<result>` | Not supported | Complex; use WebSocket for bidirectional communication |
| **targetType** | `"node"` or `"device"` attribute | Not supported | Always targets the receiving node; device-level passthrough is implementation-specific |
| **msgformat** | `msgformat` attribute on requests | `Content-Type` HTTP header | Standard HTTP content negotiation; no custom attribute needed |
| **SOAP/WSDL** | Full WSDL binding defined | Not supported | Dead weight for HTTP-native devices |
| **Callback** | `callback` URI attribute, `"0"` for same-connection | `callback` URL, or omit for WebSocket | Same concept; WebSocket replaces the `"0"` convention |
| **Authorization** | `authorization` attribute (v2.0) | HTTP `Authorization` header | Use standard HTTP mechanisms |
| **Description** | `<description lang="en">text</description>` multi-language | `"desc"` single string | Multi-language descriptions add complexity for negligible embedded benefit |

### What OMI-Lite Preserves

The core design principles that make O-MI/O-DF effective are preserved:

1. **Hierarchical object tree** with path-based addressing
2. **Core operations** (read, write, delete, cancel, response) with subscriptions as a read variant
3. **History queries** with newest/oldest/begin/end
4. **Event-based and interval-based subscriptions**
5. **HTTP callbacks** for subscription delivery
6. **TTL** on all messages
7. **Metadata** on InfoItems
8. **Semantic typing** via URI type attributes
9. **HTTP status codes** for responses
10. **REST discovery** via HTTP GET

### What OMI-Lite Drops

Features that add complexity without practical benefit for constrained P2P devices:

1. **XML encoding** — verbose, requires streaming parsers, character escaping issues
2. **SOAP/WSDL bindings** — legacy enterprise integration technology
3. **Separate O-MI + O-DF specs** — artificial division
4. **nodeList routing** — gateway/proxy concern, not device concern
5. **call operation** — write + onwrite scripts achieve the same result (proven by eOMI)
6. **altname** — multiple names for one InfoItem adds confusion
7. **IoTIdType complexity** — startDate, endDate, tagType, idType sub-attributes unused in practice
8. **O-DEF prefix system** — just use full URIs
9. **msgformat attribute** — use HTTP Content-Type header
10. **Nested omiEnvelope in response** — use WebSocket for bidirectional
11. **targetType** — always targets the node
12. **interval=0 and interval=-2** — rarely needed; 0 is a potential DoS vector
13. **Multi-language descriptions** — one string is enough for device context

---

## 11. Implementation Notes for Constrained Devices

### 11.1 Memory Budget (ESP32-S2 Reference)

Based on eOMI measurements on ESP32-S2 (320 KB SRAM, 2 MB PSRAM):

| Resource | Budget | Notes |
|----------|--------|-------|
| Static RAM | ~120 KB | Kernel, libraries, BSS |
| Heap (working) | ~25 KB | JSON parsing, connection buffers |
| PSRAM | ~2 MB | O-DF tree storage, scripts, string values |

JSON parsing requires significantly less working memory than XML streaming parsers because:
- No state machine needed for element/attribute/namespace tracking
- No character entity decoding (`&lt;` → `<`, etc.)
- Simpler tokenizer (6 structural characters vs. XML's complex grammar)

### 11.2 Recommended JSON Parser Approach

For constrained devices, use a **streaming/SAX-style JSON parser** (not DOM):
- Process tokens as they arrive
- Extract paths and values without building a full tree in memory
- Validate structure incrementally
- Reject malformed messages early

### 11.3 CBOR for Minimum Overhead

When bandwidth or parsing speed is critical, CBOR encoding reduces:
- Wire size by ~30-50% compared to JSON (no quotes, shorter type indicators)
- Parse time (binary format, no string-to-number conversion)
- Memory (smaller buffers needed)

The data model is identical — only the encoding changes.

### 11.4 Value Storage

InfoItem values should be stored in a circular buffer with configurable depth per item. Recommended defaults:
- Sensor readings: keep last 100 values
- Control states (switches, setpoints): keep last 10 values
- Events: keep last 50 values

Timestamps should use 64-bit doubles (same as JavaScript `Date.now() / 1000`) to support fractional seconds.

---

## 12. Security Considerations

OMI-Lite does not define a security mechanism but makes the following recommendations:

1. **Transport security**: Use HTTPS and WSS in production
2. **Authentication**: Use HTTP `Authorization` header (Bearer tokens, API keys)
3. **Write protection**: InfoItems should declare `writable` in metadata; servers must enforce it
4. **Rate limiting**: Servers should limit subscription creation and write frequency
5. **Path validation**: Reject paths with `..` or other traversal attempts
6. **TTL limits**: Servers may cap TTL values to prevent resource exhaustion

---

## 13. IANA Considerations

OMI-Lite uses standard MIME types:
- `application/json` (RFC 8259)
- `application/cbor` (RFC 8949)

WebSocket subprotocol identifiers:
- `omi-json`
- `omi-cbor`
