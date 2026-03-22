# Debug Plan: Post-Analysis Dashboard

## Current Issue
After Ollama returns a response, the UI shows blank screen instead of the analysis dashboard.

## Step 1: Debug Logging (DONE)
- [x] Add console logs for Ollama response
- [x] Add debug overlay showing UI state
- [ ] Test and check browser console (F12)

## Step 2: Verify Response Flow
Check these in browser console:
1. `[Underwrite] RAW RESPONSE FROM OLLAMA` - Is there a response?
2. `[Underwrite] Terminal output set` - Is terminal being populated?
3. Debug overlay - What state shows when blank?

## Step 3: Fix State Issues
Based on debug results, fix:
- [ ] If no response: Ollama integration issue
- [ ] If response but blank: Terminal panel visibility issue
- [ ] If wrong state: fileSelected/isLoading state issue

## Step 4: Build Analysis Dashboard
After debug shows response is working:
- [ ] Parse AI response (extract JSON if present)
- [ ] Display key metrics in cards (deposits, withdrawals, risk score)
- [ ] Show full analysis in expandable section
- [ ] Add "Copy Results" button

## Step 5: Add Follow-up Chat
- [ ] Chat input field below analysis
- [ ] Send follow-up questions to Ollama
- [ ] Display conversation thread
- [ ] Maintain context (same PDF, same session)

## Step 6: Polish
- [ ] Loading states for chat
- [ ] Export to JSON/CSV
- [ ] Save analysis history
- [ ] Print-friendly view

---

## Expected Terminal Output Format

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
```

The dashboard should parse this and display it nicely.

---

## Test Commands

After uploading a PDF, check browser console:
```javascript
// In browser console (F12):
console.log('File selected:', fileSelected.value)
console.log('Terminal content:', terminalOutput.value)
```

If terminal is empty but Ollama responded, the issue is state management.
If Ollama didn't respond, the issue is backend integration.
