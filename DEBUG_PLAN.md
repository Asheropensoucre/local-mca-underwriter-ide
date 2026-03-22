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

## Step 2: Verify Response Flow

Check these in the browser console (F12):
1. `[Underwrite] RAW RESPONSE FROM OLLAMA` - Is there a response?
2. `[Underwrite] Dashboard output set` - Is the right panel receiving the data?
3. Debug overlay - What state shows when blank?

---

## Step 3: Fix State Issues

Based on debug results, fix:
- [x] If no response: Ollama integration issue.
- [x] If response but blank: UI layout was unmounted. Applied persistent rendering (Step 1).
- [x] If wrong state: fileSelected/isLoading state issue.

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

## Step 6: Polish

- [ ] Add localized loading states for the chat panel (do not hide the PDF viewer)
- [ ] Export to JSON/CSV
- [ ] Save analysis history
- [ ] Print-friendly view

---

## Test Commands

After uploading a PDF, check the browser console:

```javascript
// In browser console (F12):
console.log('File selected:', fileSelected.value)
console.log('Chat/Dashboard content:', chatOutput.value)
```

**If the right panel is empty but Ollama responded:** The issue is state management (the DOM was unmounted).  
**If Ollama didn't respond:** The issue is backend integration.

---

## Debug Checklist

- [ ] Layout persists during `ANALYZING` state
- [ ] Loading spinner visible in Right Panel during analysis
- [ ] Error state displays in Right Panel with retry button
- [ ] Complete state shows dashboard cards
- [ ] JSON parsing extracts all expected fields
- [ ] Currency values formatted correctly ($XX,XXX.XX)
- [ ] Recommendation badge shows correct color
- [ ] Copy button works
