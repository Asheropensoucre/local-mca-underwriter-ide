# Debug & Pivot Plan: Post-Analysis Dashboard

## The Pivot (CRITICAL)
ABORT the "Terminal Output" UI. We are building a premium Fintech workspace (Zed/Cursor aesthetic) for bank underwriters. We need a clean dashboard and a conversational AI chat interface, not a hacker command prompt.

---

## Step 1: Layout Overhaul (The Blank Screen Fix) ✅ COMPLETE

**Status:** ✅ COMPLETED

**Changes Made:**
- Removed the "Terminal" component entirely from `App.vue`
- Locked the Left Panel (60%) to the PDF Viewer
- Locked the Right Panel (40%) to the "Underwriter Assistant" (Dashboard/Chat)
- Changed main layout from `v-if/v-else` to `v-show` so it NEVER unmounts during the `ANALYZING` state
- Added targeted loading states inside the Right Panel during analysis

**Result:** The PDF viewer and right panel remain mounted throughout all state transitions. No more blank screen crashes.

---

## Step 2: Verify Response Flow ✅ COMPLETE

**Status:** ✅ COMPLETED

Verified console logging for:
1. `[Underwrite] RAW RESPONSE FROM OLLAMA` - Response received
2. `[Underwrite] Parsed dashboard data` - Data extracted
3. Debug overlay - State transitions working correctly

---

## Step 3: Fix State Issues ✅ COMPLETE

**Status:** ✅ COMPLETED

All state issues resolved:
- [x] Ollama integration working
- [x] UI layout persists (no unmounting)
- [x] State machine transitions correct

---

## Step 4: Build Analysis Dashboard (Success State) ✅ COMPLETE

**Status:** ✅ COMPLETED

**Goal:** Parse the AI response and display key metrics in premium UI cards.

### Completed Tasks:
- [x] Parse the AI response (extract JSON if present)
  - Added `parseJsonFromResponse()` utility function
  - Handles raw JSON, markdown code blocks, and mixed prose
- [x] Display key metrics in clean, premium UI cards:
  - Business name & account info (collapsible header card)
  - Average daily balance (formatted currency)
  - Total deposits / withdrawals (green/red color coding)
  - NSF/overdraft counts (color-coded)
  - Risk score (color-coded 1-10 scale)
  - Recommendation badge (APPROVE=green, DENY=red, REVIEW=yellow)
- [x] Show the full analysis notes in an expandable section
- [x] Add a "Copy Results" button with feedback

### Implementation Details:

**JSON Parsing:**
```javascript
parseJsonFromResponse(text) // Extracts JSON from various formats
```

**Dashboard Cards:**
- Business Information card (name, account, period)
- Metrics grid (4 cards): Avg Balance, Deposits, Withdrawals, NSF Count
- Risk & Recommendation row (2 cards): Risk Score, Recommendation with indicator dot
- Analysis Notes section with Copy button

**Utility Functions:**
- `formatCurrency(value)` - USD formatting
- `getRecommendationColor(rec)` - Badge colors
- `getRiskScoreColor(score)` - Risk score colors
- `copyResults()` - Clipboard copy with feedback

### Expected JSON Format:
```json
{
  "business": {
    "name": "ABC Company LLC",
    "account": "****4521",
    "period": "01/2024 - 01/2024"
  },
  "metrics": {
    "avg_daily_balance": 45230.50,
    "total_deposits": 285000.00,
    "total_withdrawals": 267000.00,
    "nsf_count": 0
  },
  "risk": {
    "overdrafts": 0,
    "score": 8
  },
  "recommendation": "APPROVE",
  "notes": "Strong cash flow, no risk indicators..."
}
```

**Fallback Behavior:** If JSON parsing fails, displays raw AI response in the notes section.

---

## Step 5: Add Follow-up Chat ✅ COMPLETE

**Status:** ✅ COMPLETED

**Goal:** Add conversational follow-up questions about the analyzed PDF.

### Completed Tasks:
- [x] Add a sleek Chat input field below the analysis cards
- [x] Send follow-up questions to Ollama
- [x] Display the conversation thread above the input
- [x] Maintain context (same PDF, same session)

### Implementation Details:

**Chat UI Components:**
- Chat header ("Follow-up Questions")
- Message thread (scrollable, max-h-64)
  - User messages: Blue background with primary border
  - Assistant messages: Surface background with border
  - Loading state: Spinner with "Thinking..."
- Input field with Enter key support
- Send button (disabled when loading/disconnected)
- Empty state: "Ask follow-up questions about this analysis"

**State Management:**
```javascript
chatMessages = ref([])      // Array of { role, content }
chatInput = ref('')         // Current input
isChatLoading = ref(false)  // Loading indicator
```

**Context Preservation:**
- Includes full previous analysis in prompt
- Sends PDF path for vision model reference
- Maintains session context for follow-ups

**Chat Function:**
```javascript
sendChatMessage() {
  // 1. Add user message to thread
  // 2. Build context-aware prompt with previous analysis
  // 3. Send to Ollama with same PDF
  // 4. Add assistant response to thread
  // 5. Handle errors gracefully
}
```

**User Experience:**
- Press Enter to send
- Disabled state when Ollama disconnected
- Loading spinner during response generation
- Error messages displayed in chat thread

---

## Step 6: Polish ✅ COMPLETE

**Status:** ✅ COMPLETED

**Goal:** Add finishing touches and quality-of-life features.

### Completed Tasks:
- [x] Export analysis to JSON/CSV
- [x] Print-friendly view
- [x] Localized loading states for chat panel (already implemented in Step 5)

### Implementation Details:

#### 6a. Export to JSON/CSV ✅

**JSON Export:**
- Button: "📄 JSON" in Analysis Notes header
- Saves full parsed JSON structure
- User chooses save location via native dialog
- File format: `analysis-{filename}.json`

**CSV Export:**
- Button: "📊 CSV" in Analysis Notes header
- Structured sections: Business Info, Financial Metrics, Risk Assessment, Recommendation, Notes
- Proper CSV escaping for special characters
- File format: `analysis-{filename}.csv`

**Export Functions:**
```javascript
exportToJSON() // Saves parsed JSON to user-selected location
exportToCSV()  // Saves formatted CSV with sections
```

**State Management:**
```javascript
isExporting = ref(false) // Disabled during export
```

#### 6b. Print-Friendly View ✅

**Features:**
- Button: "🖨️ Print" in Analysis Notes header
- Generates clean HTML report with light theme
- Sections: Business Info, Financial Metrics, Risk Assessment, Analysis Notes
- Color-coded recommendation badge (green/yellow/red)
- Opens browser print dialog for PDF or physical print
- Includes timestamp and filename

**Print Layout:**
- Professional white background
- Grid-based card layout
- Proper typography and spacing
- Print media query optimization
- Footer with app attribution

**Print Function:**
```javascript
printReport() {
  // Generates styled HTML report
  // Opens print window
  // Triggers browser print dialog
}
```

#### 6c. Loading States ✅

Already implemented in Step 5:
- Chat panel has dedicated loading spinner
- Export buttons disabled during export
- Visual feedback for all async operations

---

## Debug Checklist (Final)

- [x] Layout persists during `ANALYZING` state
- [x] Loading spinner visible in Right Panel during analysis
- [x] Error state displays in Right Panel with retry button
- [x] Complete state shows dashboard cards
- [x] JSON parsing extracts all expected fields
- [x] Currency values formatted correctly ($XX,XXX.XX)
- [x] Recommendation badge shows correct color
- [x] Copy button works
- [x] Chat input appears after analysis
- [x] Chat messages send and display
- [x] Chat maintains context with previous analysis
- [x] Export JSON button works
- [x] Export CSV button works
- [x] Print report opens print dialog
- [x] All buttons disabled during loading states

---

## Test Commands

After uploading a PDF, check the browser console:

```javascript
// In browser console (F12):
console.log('File selected:', fileSelected.value)
console.log('Chat/Dashboard content:', chatOutput.value)
console.log('Parsed data:', parsedData.value)
```

**If the right panel is empty but Ollama responded:** The issue is state management (the DOM was unmounted).  
**If Ollama didn't respond:** The issue is backend integration.

---

## Debug Checklist

- [x] Layout persists during `ANALYZING` state
- [x] Loading spinner visible in Right Panel during analysis
- [x] Error state displays in Right Panel with retry button
- [x] Complete state shows dashboard cards
- [x] JSON parsing extracts all expected fields
- [x] Currency values formatted correctly ($XX,XXX.XX)
- [x] Recommendation badge shows correct color
- [x] Copy button works
- [x] Chat input appears after analysis
- [x] Chat messages send and display
- [x] Chat maintains context with previous analysis
