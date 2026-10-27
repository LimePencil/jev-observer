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

INPUT_FIELDS = ('state', 'input', 'images', 'image', 'audio', 'video', 'videos', 'media', 'assets')


def object_value(value, description):
    if not isinstance(value, dict):
        raise ValueError(f'{description} must be an object')
    return value


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('Duplicate JSON object keys would lose original correspondence')
        result[key] = value
    return result


def named_results(response, dialect, state_id=None):
    object_value(response, 'Response')
    if dialect == 'omnijev':
        # MSO returns the named mapping itself; a question named "answers" is
        # not an envelope and must remain an ordinary question.
        answers = response
    elif dialect in ('nanojev', 'agentjev'):
        items = response['states' if dialect == 'nanojev' else 'results']
        if not isinstance(items, list) or any(not isinstance(item, dict) for item in items):
            raise ValueError('Batch results must be an array of objects')
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
        if any(not isinstance(item, dict) or not isinstance(item.get('id'), str) for item in answers):
            raise ValueError('Result arrays require objects with string IDs')
        mapped = {item['id']: item for item in answers}
        if len(mapped) != len(answers):
            raise ValueError('Duplicate question IDs')
        return mapped
    if not isinstance(answers, dict):
        raise ValueError('Answers must be named objects or objects with explicit IDs')
    return answers


def map_jev_omni(request, response):
    """Map one saved classifier call using the original ordered option IDs."""
    questions = request['questions']
    if len(questions) != 1:
        raise ValueError('Jev-Omni represents one question per original call')
    key, question = next(iter(questions.items()))
    criteria = question.get('criteria')
    if question.get('type') != 'choice' or not isinstance(criteria, dict) or not 2 <= len(criteria) <= 255:
        raise ValueError('Jev-Omni needs an original Choice with 2 to 255 ordered options')
    labels = list(criteria)
    probabilities = response.get('probabilities')
    if not isinstance(probabilities, dict) or list(probabilities) != labels:
        raise ValueError('Jev-Omni probability keys must match the original option order exactly')
    index = response.get('prediction_index')
    if type(index) is not int or not 0 <= index < len(labels) or response.get('prediction') != labels[index]:
        raise ValueError('Jev-Omni prediction and index must identify the same original option')
    answer = {'type': 'choice', 'choice': response['prediction'],
              'probabilities': copy.deepcopy(probabilities),
              # This dialect's result is the response root. Recognized input
              # fields are copied once to that envelope by convert(), where
              # Observer's input-capture preference can remove them.
              'original_result': {k: copy.deepcopy(v) for k, v in response.items() if k not in INPUT_FIELDS}}
    if 'confidence' in response:
        answer['confidence'] = response['confidence']
    return {key: answer}


def map_omnijev_score(question, raw, answer):
    """Map explicit native level IDs to indices, retaining modal semantics."""
    levels = question.get('criteria')
    if (not isinstance(levels, list) or not 1 <= len(levels) <= 255
            or any(not isinstance(level, str) for level in levels) or len(set(levels)) != len(levels)):
        raise ValueError('OmniJev Score needs distinct original string level IDs in rubric order')
    probabilities = raw.get('probabilities')
    if not isinstance(probabilities, dict):
        raise ValueError('OmniJev Score requires a named probability map')
    # Native invalid/no-renderable-options results remain inspectable invalid
    # observations. Their absent winning level is never repaired or guessed.
    if not probabilities and raw.get('score') is None:
        answer['score'] = None
        answer['probabilities'] = {}
    else:
        if set(probabilities) != set(levels) or raw.get('score') not in levels:
            raise ValueError('OmniJev Score keys must exactly match the original ordered level IDs')
        answer['score'] = levels.index(raw['score'])
        answer['probabilities'] = {str(i): probabilities[level] for i, level in enumerate(levels)}
    answer['mapping_note'] = 'Reported modal level mapped to its original ordinal index; no expected score computed'


def map_answers(request, response, dialect, state_id=None):
    object_value(request, 'Request')
    object_value(response, 'Response')
    object_value(request.get('questions'), 'Named questions')
    for question in request['questions'].values():
        object_value(question, 'Question definition')
    if dialect == 'jev-omni':
        return map_jev_omni(request, response)
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
        object_value(question, 'Question definition')
        raw = object_value(answers[key], 'Answer')
        kind = question['type']
        if kind not in ('choice', 'score', 'noul'):
            raise ValueError('Use choice, score or noul definitions')
        answer = {'type': kind, 'original_result': copy.deepcopy(raw)}
        if dialect == 'omnijev':
            for field in ('abstain', 'valid'):
                if field in raw:
                    answer[field] = copy.deepcopy(raw[field])
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
                # Deriving a winner requires every original option and numeric
                # evidence. Preserve explicit provider answers for downstream
                # validation, but never invent one from a partial/invalid map.
                criteria = question.get('criteria')
                if (not isinstance(criteria, dict) or not criteria or dist.keys() != criteria.keys()
                        or any(type(value) not in (int, float) or not 0 <= value <= 1
                               for value in dist.values())):
                    raise ValueError('SemIf argmax requires the complete original option IDs '
                                     'and finite numeric probabilities between zero and one')
                choice = max(dist, key=dist.get)
                answer['mapping_note'] = 'Choice is argmax of the complete reported option distribution'
            if choice is None and dialect != 'omnijev':
                raise ValueError('Choice needs a reported option ID')
            answer['choice'] = choice
        else:
            if dialect == 'omnijev':
                map_omnijev_score(question, raw, answer)
                mapped[key] = answer
                continue
            if 'score' not in raw:
                raise ValueError('Score needs a reported ordinal expected level; map numeric bins explicitly')
            answer['score'] = raw['score']
            if 'legend' in raw:
                legend = raw['legend']
                answer['legend'] = {str(i): v for i, v in enumerate(legend)} if isinstance(legend, list) else legend
        mapped[key] = answer
    return mapped


def convert(request, response, *, dialect, event_id, timestamp, source, provider, status=200,
            duration_ms=None, state_id=None, sample=False, model_profile=None):
    object_value(request, 'Request')
    object_value(response, 'Response')
    if not isinstance(request.get('questions'), dict):
        raise ValueError('Request must contain original named System One definitions')
    for question in request['questions'].values():
        object_value(question, 'Question definition')
    if dialect == 'systemone':
        output = copy.deepcopy(response)
    else:
        output = {'answers': map_answers(request, response, dialect, state_id)}
        if isinstance(response.get('model'), str):
            output['model'] = response['model']
        # Custom "usage" often counts paths or questions. Preserve it as metadata,
        # not as billed input/output tokens. A local import never estimates usage.
        output['original_metadata'] = {} if dialect == 'omnijev' else {
            k: v for k, v in response.items()
            if k not in ('answers', 'questions', 'states', 'results') + INPUT_FIELDS}
        # Keep input snapshots where Observer's capture-state preference applies.
        # Nesting them in arbitrary metadata would conceal them from that filter.
        for field in (() if dialect == 'omnijev' else INPUT_FIELDS):
            if field in response:
                output[field] = copy.deepcopy(response[field])
    result = {'id': event_id, 'timestamp': timestamp, 'source': source, 'provider': provider,
            'status': status, 'duration_ms': duration_ms, 'capture_complete': True,
            'sample': sample, 'request': request, 'response': output}
    if model_profile is None and dialect in ('omnijev', 'jev-omni'):
        model_profile = dialect
    if model_profile is not None:
        result.update(model_profile=model_profile, protocol='systemone')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--request', type=Path, required=True)
    parser.add_argument('--response', type=Path, required=True)
    parser.add_argument('--dialect', choices=['systemone', 'nanojev', 'agentjev', 'anyjev', 'semif', 'autotrust', 'omnijev', 'jev-omni'], default='systemone')
    parser.add_argument('--id', required=True, help='Stable source event ID; reuse only for the same call')
    parser.add_argument('--timestamp', type=int, required=True, help='Original event time in Unix milliseconds')
    parser.add_argument('--source', required=True)
    parser.add_argument('--provider', required=True, help='Catalog provider ID, such as nanojev or autotrust')
    parser.add_argument('--model-profile', help='Validation profile independent of provider attribution; Omni dialects default to their own profile')
    parser.add_argument('--status', type=int, default=200)
    parser.add_argument('--duration-ms', type=float)
    parser.add_argument('--state-id')
    parser.add_argument('--sample', action='store_true')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    try:
        request = json.loads(args.request.read_text(encoding='utf-8'), object_pairs_hook=unique_object)
        response = json.loads(args.response.read_text(encoding='utf-8'), object_pairs_hook=unique_object)
        result = convert(request, response, dialect=args.dialect, event_id=args.id,
                         timestamp=args.timestamp, source=args.source, provider=args.provider,
                         status=args.status, duration_ms=args.duration_ms, state_id=args.state_id,
                         sample=args.sample, model_profile=args.model_profile)
        # Validate and encode before creating the file so bad input cannot leave
        # an empty capture that prevents a corrected conversion from succeeding.
        payload = (json.dumps(result, ensure_ascii=False, allow_nan=False) + '\n').encode('utf-8')
        # Refuse overwrite so an earlier collection cannot be lost by rerunning.
        with args.output.open('xb') as output:
            output.write(payload)
    except (ValueError, KeyError, TypeError, OSError) as error:
        parser.exit(1, f'Capture conversion failed: {error}\n')


if __name__ == '__main__':
    main()
