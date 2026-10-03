#!/usr/bin/env python3
"""Map saved decision results to Observer captures without inference or network access.

The request file must contain the exact named System One definitions corresponding
one-to-one with the saved results. Mapping definitions is an application decision;
this script does not infer task meaning, confidence, latency, cost or token usage.
"""
import argparse
import copy
import json
from pathlib import Path


def named_results(response, dialect, state_id=None):
    if dialect in ('nanojev', 'agentjev'):
        items = response['states' if dialect == 'nanojev' else 'results']
        if state_id is None:
            if len(items) != 1:
                raise ValueError('Batch results require --state-id; no rows are pooled')
            item = items[0]
        else:
            matches = [item for item in items if item.get('id') == state_id]
            if len(matches) != 1:
                raise ValueError('State ID must identify exactly one result')
            item = matches[0]
        answers = item['answers']
    elif dialect == 'anyjev':
        answers = response['questions']
    else:
        answers = response.get('answers', response)
    if isinstance(answers, list):
        mapped = {item['id']: item for item in answers}
        if len(mapped) != len(answers):
            raise ValueError('Duplicate question IDs')
        return mapped
    if not isinstance(answers, dict):
        raise ValueError('Answers must be named objects or objects with explicit IDs')
    return answers


def map_answers(request, response, dialect, state_id=None):
    if dialect in ('autotrust', 'semif') and 'probabilities' in response and len(request['questions']) == 1:
        key = next(iter(request['questions']))
        if response.get('id', key) != key:
            raise ValueError('Result ID differs from the original question ID')
        answers = {key: response}
    else:
        answers = named_results(response, dialect, state_id)
    if answers.keys() != request['questions'].keys():
        raise ValueError('Result IDs must exactly match the original question IDs')
    mapped = {}
    for key, question in request['questions'].items():
        raw = answers[key]
        kind = question['type']
        if kind not in ('choice', 'score', 'noul'):
            raise ValueError('Use choice, score or noul definitions')
        answer = {'type': kind, 'original_result': copy.deepcopy(raw)}
        dist = raw.get('probabilities', raw.get('distribution'))
        if isinstance(dist, list):
            # Array positions have meaning only with explicit option IDs.
            options = raw.get('option_ids', raw.get('options'))
            if not isinstance(options, list) or len(options) != len(dist):
                raise ValueError('Probability arrays require equally sized explicit option IDs')
            labels = [str(option) for option in options]
            if len(labels) != len(set(labels)):
                raise ValueError('Duplicate option IDs')
            dist = dict(zip(labels, dist))
        if dist is not None:
            answer['probabilities'] = dist
        if 'confidence' in raw:
            answer['confidence'] = raw['confidence']
        if kind == 'noul':
            # Never map a thresholded boolean as a probability.
            for field in ('noul', 'p_true', 'probability'):
                if field in raw:
                    answer['noul'] = raw[field]
                    break
            else:
                if isinstance(dist, dict) and 'true' in dist:
                    answer['noul'] = dist['true']
                else:
                    raise ValueError('Noul needs a reported probability of true')
        elif kind == 'choice':
            choice = raw.get('choice', raw.get('answer', raw.get('value')))
            if choice is None and dialect == 'semif' and isinstance(dist, dict):
                choice = max(dist, key=dist.get)
                answer['mapping_note'] = 'Choice is argmax of the complete reported option distribution'
            if choice is None:
                raise ValueError('Choice needs a reported option ID')
            answer['choice'] = choice
        else:
            if 'score' not in raw:
                raise ValueError('Score needs a reported ordinal expected level; map numeric bins explicitly')
            answer['score'] = raw['score']
            if 'legend' in raw:
                legend = raw['legend']
                answer['legend'] = {str(i): v for i, v in enumerate(legend)} if isinstance(legend, list) else legend
        mapped[key] = answer
    return mapped


def convert(request, response, *, dialect, event_id, timestamp, source, provider, status=200,
            duration_ms=None, state_id=None, sample=False):
    if not isinstance(request.get('questions'), dict):
        raise ValueError('Request must contain original named System One definitions')
    if dialect == 'systemone':
        output = copy.deepcopy(response)
    else:
        output = {'answers': map_answers(request, response, dialect, state_id)}
        if isinstance(response.get('model'), str):
            output['model'] = response['model']
        # Custom "usage" often counts paths or questions. Preserve it as metadata,
        # not as billed input/output tokens. A local import never estimates usage.
        output['original_metadata'] = {k: v for k, v in response.items()
                                       if k not in ('answers', 'questions', 'states', 'results')}
    return {'id': event_id, 'timestamp': timestamp, 'source': source, 'provider': provider,
            'status': status, 'duration_ms': duration_ms, 'capture_complete': True,
            'sample': sample, 'request': request, 'response': output}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--request', type=Path, required=True)
    parser.add_argument('--response', type=Path, required=True)
    parser.add_argument('--dialect', choices=['systemone', 'nanojev', 'agentjev', 'anyjev', 'semif', 'autotrust'], default='systemone')
    parser.add_argument('--id', required=True, help='Stable source event ID; reuse only for the same call')
    parser.add_argument('--timestamp', type=int, required=True, help='Original event time in Unix milliseconds')
    parser.add_argument('--source', required=True)
    parser.add_argument('--provider', required=True, help='Catalog provider ID, such as nanojev or autotrust')
    parser.add_argument('--status', type=int, default=200)
    parser.add_argument('--duration-ms', type=float)
    parser.add_argument('--state-id')
    parser.add_argument('--sample', action='store_true')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    try:
        request = json.loads(args.request.read_text())
        response = json.loads(args.response.read_text())
        result = convert(request, response, dialect=args.dialect, event_id=args.id,
                         timestamp=args.timestamp, source=args.source, provider=args.provider,
                         status=args.status, duration_ms=args.duration_ms, state_id=args.state_id,
                         sample=args.sample)
        # Refuse overwrite so an earlier collection cannot be lost by rerunning.
        with args.output.open('x') as output:
            output.write(json.dumps(result, ensure_ascii=False, allow_nan=False) + '\n')
    except (ValueError, KeyError, TypeError, OSError) as error:
        parser.exit(1, f'Capture conversion failed: {error}\n')


if __name__ == '__main__':
    main()
