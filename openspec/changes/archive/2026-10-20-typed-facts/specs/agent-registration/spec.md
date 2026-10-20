## MODIFIED Requirements

### Requirement: Agent facts are a separate self-authored channel
The registry SHALL accept a separate self-authored `facts` channel for model, provider, thinking, context window usage, session display name and unresolved background tasks. Context window usage SHALL be `context_window` carrying integer `tokens` in context and integer `window` size, where `window` is greater than zero and `tokens` does not exceed it. It SHALL use independent writer fencing, sequence and freshness and SHALL NOT merge facts with execution or assignment.

#### Scenario: Facts without an owner
- **WHEN** an ordinary session registers and publishes facts
- **THEN** the registry exposes them with that publisher's provenance and requires no owner or assignment

#### Scenario: Unknown field
- **WHEN** a facts snapshot contains a field the contract does not define
- **THEN** it is refused and the previous snapshot is unchanged

#### Scenario: Context usage is typed
- **WHEN** a publisher reports 86000 tokens in a 200000-token window
- **THEN** reads expose both integers and a display derives 43% from them

#### Scenario: Invalid context window
- **WHEN** a snapshot has a zero window, tokens above the window, a negative count or text in place of an integer
- **THEN** it is refused and the previous snapshot is unchanged
