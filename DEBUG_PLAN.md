# Debug & Pivot Plan: Post-Analysis Dashboard

## Current Issue
After Ollama returns a response, the UI shows a blank screen instead of the analysis dashboard. This is caused by the state machine destroying (unmounting) the layout during the loading phase and failing to rebuild the heavy PDF components.

## The Pivot (CRITICAL)
ABORT the "Terminal Output" UI. We are building a premium Fintech workspace (Zed/Cursor aesthetic) for bank underwriters. We need a clean dashboard and a conversational AI chat interface, not a hacker command prompt.

## Step 1: Layout Overhaul (The Blank Screen Fix)
- [ ] Remove the "Terminal" component entirely from `App.vue`.
- [ ] Lock the Left Panel (60%) to the PDF Viewer.
- [ ] Lock the Right Panel (40%) to the "Underwriter Assistant" (Dashboard/Chat).
- [ ] Ensure the main layout uses `v-show` or persistent rendering so it NEVER unmounts during the `ANALYZING` state. Do not use `v-if`/`v-else` blocks that destroy the main containers.

## Step 2: Verify Response Flow
Check these in the browser console (F12):
1. `[Underwrite] RAW RESPONSE FROM OLLAMA` - Is there a response?
2. `[Underwrite] Dashboard output set` - Is the right panel receiving the data?
3. Debug overlay - What state shows when blank?

## Step 3: Fix State Issues
Based on debug results, fix:
- [ ] If no response: Ollama integration issue.
- [ ] If response but blank: UI layout was unmounted. Apply persistent rendering (Step 1).
- [ ] If wrong state: fileSelected/isLoading state issue.

## Step 4: Build Analysis Dashboard (Success State)
After debug shows the response is working and layout persists:
- [ ] Parse the AI response (extract JSON if present).
- [ ] Display key metrics in clean, premium UI cards (deposits, withdrawals, risk score) inside the right panel.
- [ ] Show the full analysis in an expandable section.
- [ ] Add a "Copy Results" button.

## Step 5: Add Follow-up Chat
- [ ] Add a sleek Chat input field below the analysis cards.
- [ ] Send follow-up questions to Ollama.
- [ ] Display the conversation thread above the input.
- [ ] Maintain context (same PDF, same session).

## Step 6: Polish
- [ ] Add localized loading states for the chat panel (do not hide the PDF viewer).
- [ ] Export to JSON/CSV.
- [ ] Save analysis history.
- [ ] Print-friendly view.

---

## Expected AI JSON Output Format

The AI should return something like:

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
The app should parse this JSON and display it nicely in the UI stat cards.

Test Commands
After uploading a PDF, check the browser console:

JavaScript
// In browser console (F12):
console.log('File selected:', fileSelected.value)
console.log('Chat/Dashboard content:', chatOutput.value)
If the right panel is empty but Ollama responded, the issue is state management (the DOM was unmounted).
If Ollama didn't respond, the issue is backend integration.