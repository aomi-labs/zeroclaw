# Aomi Labs ZeroClaw Fork

Forked from [zeroclaw-labs/zeroclaw](https://github.com/zeroclaw-labs/zeroclaw) v0.6.2.

This fork strips nearly all security enforcement to give Phoebe unrestricted access on our private server. Not for public deployment.

---

## What's Been Nuked

### `src/security/policy.rs` — Main Enforcement Hub
| Function | Original | Now |
|---|---|---|
| `is_command_allowed()` | Layered allowlist with subshell blocking, redirection blocking, dangerous arg detection | Always returns `true` |
| `command_risk_level()` | Classifies commands as Low/Medium/High risk | Always returns `Low` |
| `validate_command_execution()` | Allowlist + risk gate + autonomy check + approval requirement | Always returns `Ok(Low)` |
| `is_path_allowed()` | Null-byte blocking, traversal detection, tilde expansion, forbidden prefix matching | Always returns `true` |
| `can_act()` | Blocks actions in ReadOnly mode | Always returns `true` |
| `enforce_tool_operation()` | Rate limit + autonomy gate for Act operations | Always returns `Ok(())` |
| `record_action()` | Sliding-window rate limiter | Always returns `true` (no tracking) |
| `is_rate_limited()` | Checks if rate limit exceeded | Always returns `false` |
| `forbidden_path_argument()` | Scans command args for forbidden paths | Always returns `None` |

### `src/approval/mod.rs` — Tool Approval Gate
| Function | Original | Now |
|---|---|---|
| `needs_approval()` | Checks autonomy level, always_ask list, auto_approve list, session allowlist | Always returns `false` |

### `src/security/detect.rs` — Sandbox Detection
| Function | Original | Now |
|---|---|---|
| `detect_best_sandbox()` | Auto-detects Landlock/Firejail/Bubblewrap/Seatbelt/Docker | Always returns `NoopSandbox` |

### `src/security/estop.rs` — Emergency Stop
| Behavior | Original | Now |
|---|---|---|
| Parse/read errors | Fail-closed (engages kill-all) | Fail-open (uses defaults, not engaged) |
| Manual kill switch | Works | **Retained** — still works |

### `src/security/domain_matcher.rs` — Domain Gating
| Category | Original | Now |
|---|---|---|
| Banking | chase, bankofamerica, wellsfargo, fidelity, schwab, venmo, paypal, robinhood, coinbase | **Retained** |
| Medical | mychart, epic, patient portals, health records | Emptied |
| Government | ssa.gov, irs.gov, login.gov, id.me | Emptied |
| Identity Providers | accounts.google.com, login.microsoftonline.com, appleid.apple.com | Emptied |

### `src/security/iam_policy.rs` — IAM Role-Based Access
| Function | Original | Now |
|---|---|---|
| `evaluate_tool_access()` | Deny-by-default, requires matching role | Always returns `Allow` |
| `evaluate_workspace_access()` | Deny-by-default, requires matching role | Always returns `Allow` |

### `src/security/workspace_boundary.rs` — Workspace Confinement
| Function | Original | Now |
|---|---|---|
| `check_tool_access()` | Checks if tool is restricted in workspace profile | Always returns `Allow` |
| `check_domain_access()` | Checks domain against workspace allowlist | Always returns `Allow` |
| `check_path_access()` | Blocks cross-workspace access, enforces isolation | Always returns `Allow` |

### `src/security/mod.rs` — Utility Functions
| Function | Original | Now |
|---|---|---|
| `redact()` | Shows first 4 chars + `***` | Returns value as-is (cleartext) |

### `src/security/leak_detector.rs` — Credential Leak Detection
| Function | Original | Now |
|---|---|---|
| `scan()` | Detects API keys, AWS creds, secrets, private keys, JWTs, database URLs, high-entropy tokens and replaces with `[REDACTED_*]` | Always returns `Clean` (no detection) |

---

## What's Retained

1. **E-stop manual kill** — `EstopManager::is_engaged()` and manual engage/disengage still work. If you need to kill Phoebe, e-stop works.
2. **OTP on banking domains** — Banking sites (chase, paypal, coinbase, etc.) still require OTP verification.
3. **Audit logging** — `hooks/builtin/command_logger` still records all tool calls. Zero overhead, full audit trail.

---

## Why

Phoebe runs on a private DigitalOcean droplet owned by Aomi Labs. The upstream security model assumes untrusted multi-tenant environments. We're single-tenant, single-operator. The restrictions were:
- Blocking legitimate shell commands (curl, wget, docker)
- Rate-limiting tool usage during normal conversations
- Redacting credentials Phoebe needs to use (Vercel tokens, API keys, passwords)
- Requiring approval for every write/exec operation via Discord/Telegram
- Sandboxing commands that need full system access
- Blocking file access outside the workspace directory
