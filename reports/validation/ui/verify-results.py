import json
from pathlib import Path
r=json.loads((Path(__file__).parent/'results.json').read_text())
assert len(r['clipboard'])==8
assert len(r['deletion'])==6
for case in r['clipboard']:
    after=case['label']=='after'
    missing=case['mode']=='unavailable'
    allowed=case['mode']=='allowed'
    assert not case['unhandledRejections'],case
    assert bool(case['pageErrors']) == (missing and not after),case
    assert bool(case['exposedCopyFeedback']) == after,case
    if missing and not after:
        assert case['feedback']==[],case
        assert 'writeText' in case['pageErrors'][0]['message'],case
    else:
        feedback=case['feedback'][0]
        assert feedback['insideDialog']==after,case
        assert bool(feedback['ariaHiddenAncestors']) != after,case
        assert feedback['text']==('Local base URL copied.' if allowed else 'Could not copy. Select the URL to copy it manually.'),case
    if allowed:
        assert case['initial']['writePermission']=='granted',case
        assert case['readback']==('http://127.0.0.1:19863' if after else 'http://127.0.0.1:19861'),case
    if case['mode']=='denied':
        assert case['initial']['writePermission']=='denied',case
for case in r['deletion']:
    assert case['originalWasDeleting'],case
    assert case['responseStatus']==200 and json.loads(case['responseText'])=={'ok':True},case
    assert case['beforeRelease']['dialogs']==1,case
    expected_stays=case['label']=='after' and case['nextPanel']!='stay'
    assert case['afterRelease']['dialogs']==int(expected_stays),case
    if expected_stays:
        assert case['beforeRelease']['heading']==case['afterRelease']['heading'],case
    assert not case['pageErrors'],case
print('14 independently recorded UI scenarios match the reported before/after behavior, including native clipboard success and staying-in-Settings controls.')
