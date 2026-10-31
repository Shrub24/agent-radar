## MODIFIED Requirements

### Requirement: Keyboard selection and folding
Radar SHALL support local keyboard selection and shared expanding/collapsing of branches using Vim-style keys and arrows. Refreshes SHALL preserve each view's selected identity where visible. A hidden or removed selection SHALL reconcile locally to a valid visible row, or become empty. Viewport and details state SHALL remain local. Browsing SHALL NOT request terminal focus.

#### Scenario: Navigate and fold
- **WHEN** one view selects an owner and collapses its branch
- **THEN** the shared listing reflects the fold and each frontend independently reconciles its own selection

#### Scenario: Stable refresh
- **WHEN** facts refresh without removing the selected identity from the listing
- **THEN** the view keeps that selection unless its enabled follow policy adopts changed focus

#### Scenario: Independent viewport and selection
- **WHEN** one frontend scrolls, selects another row or changes dimensions
- **THEN** another frontend's selection and viewport are unchanged

### Requirement: Text filtering
Radar SHALL support a shared live case-insensitive fuzzy filter over displayed labels and available role/assignment text, edited by at most one attachment. Matching descendants SHALL retain the ancestry needed to understand placement and tree ordering. Clearing the filter SHALL restore the otherwise eligible listing and prior folds. Each frontend SHALL reconcile its own selection against the resulting canonical rows.

#### Scenario: Match a nested assignment
- **WHEN** the filter matches a worker assignment but not its owner's label
- **THEN** the worker and necessary ancestry remain visible in tree order

#### Scenario: Clear filter
- **WHEN** the editor clears the filter
- **THEN** otherwise eligible rows return with prior folds preserved

#### Scenario: Live shared query
- **WHEN** the editor changes the query without committing
- **THEN** all frontends receive the same filter and canonical rows but not a shared browsing selection
