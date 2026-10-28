#!/usr/bin/env python3
"""Contract checks for saved custom decision results; no model dependencies."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('capture', Path(__file__).with_name('capture-decision.py'))
capture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capture)


def request():
    return {'state': 'test input', 'questions': {
        'route': {'type': 'choice', 'instructions': 'Route', 'criteria': {'a': 'A', 'b': 'B'}},
        'check': {'type': 'noul', 'instructions': 'Check'},
        'rating': {'type': 'score', 'instructions': 'Rate', 'criteria': ['Low', 'High']}}}


class CaptureTests(unittest.TestCase):
    def convert(self, original, response, dialect, **extra):
        return capture.convert(original, response, dialect=dialect, event_id='event-1',
                               timestamp=1, source='app', provider='local-provider', **extra)

    def test_omnijev_preserves_abstention_even_below_rounding_tolerance(self):
        original = {'questions': {'answers': {'type': 'choice', 'instructions': 'Route',
                                             'criteria': {'a': 'A', 'b': 'B'}}}}
        for abstain, valid in ((.2, True), (.00001, True), (0, False)):
            raw = {'answers': {'choice': 'a', 'probabilities': {'a': .5, 'b': .5 - abstain},
                               'confidence': .3, 'abstain': abstain, 'valid': valid}}
            out = self.convert(original, raw, 'omnijev')
            answer = out['response']['answers']['answers']
            self.assertEqual(answer['probabilities'], raw['answers']['probabilities'])
            self.assertEqual(answer['abstain'], abstain)
            self.assertIs(answer['valid'], valid)
            self.assertEqual(answer['original_result'], raw['answers'])
            self.assertEqual(out['response']['original_metadata'], {})
            self.assertEqual(out['provider'], 'local-provider')
            self.assertEqual(out['model_profile'], 'omnijev')
            self.assertEqual(out['protocol'], 'systemone')

    def test_omnijev_modal_score_uses_explicit_original_level_order(self):
        original = {'questions': {'q': {'type': 'score', 'instructions': 'Rate',
                                       'criteria': ['high', 'low', 'medium']}}}
        raw = {'q': {'score': 'medium', 'probabilities': {'low': .1, 'medium': .7, 'high': .2},
                     'confidence': .55, 'latency_s': .004}}
        out = self.convert(original, raw, 'omnijev')['response']['answers']['q']
        self.assertEqual(out['score'], 2)
        self.assertEqual(out['probabilities'], {'0': .2, '1': .1, '2': .7})
        self.assertEqual(out['original_result'], raw['q'])
        self.assertNotIn('expected', out)
        self.assertNotIn('legend', out)
        self.assertIn('modal', out['mapping_note'])

    def test_omnijev_missing_results_remain_invalid_without_invented_choices(self):
        for kind, criteria in (('choice', {'a': 'A', 'b': 'B'}), ('score', ['low', 'high'])):
            original = {'questions': {'q': {'type': kind, 'instructions': 'Decide', 'criteria': criteria}}}
            raw = {'q': {'choice': None, 'score': None, 'probabilities': {},
                         'abstain': 1.0, 'valid': False, 'confidence': 0.0}}
            out = self.convert(original, raw, 'omnijev')['response']['answers']['q']
            self.assertIsNone(out[kind])
            self.assertEqual(out['probabilities'], {})
            self.assertFalse(out['valid'])
            self.assertEqual(out['abstain'], 1)

    def test_omnijev_rejects_unknown_or_ambiguous_score_correspondence(self):
        raw = {'q': {'score': 'low', 'probabilities': {'low': .8, 'high': .2}, 'confidence': .6}}
        for levels in (['low', 'low'], ['low', 'other'], [{'text': 'low'}, 'high'], list(map(str, range(256)))):
            original = {'questions': {'q': {'type': 'score', 'instructions': 'Rate', 'criteria': levels}}}
            with self.assertRaises(ValueError): self.convert(original, raw, 'omnijev')

    def test_omnijev_noul_does_not_use_threshold_or_latency_as_capture_duration(self):
        original = {'questions': {'q': {'type': 'noul', 'instructions': 'Check'}}}
        out = self.convert(original, {'q': {'noul': .7345, 'latency_s': .1}}, 'omnijev')
        self.assertEqual(out['response']['answers']['q']['noul'], .7345)
        self.assertIsNone(out['duration_ms'])
        self.assertNotIn('usage', out['response'])

    def test_jev_omni_preserves_map_values_and_exact_option_correspondence(self):
        original = {'questions': {'q': {'type': 'choice', 'instructions': 'Choose',
                                       'criteria': {'β': 'Beta', 'α': 'Alpha'}}}}
        raw = {'prediction': 'α', 'prediction_index': 1, 'confidence': 2 / 3,
               'probabilities': {'β': 1 / 3, 'α': 2 / 3}}
        out = self.convert(original, raw, 'jev-omni')
        answer = out['response']['answers']['q']
        self.assertEqual(answer['choice'], 'α')
        self.assertEqual(answer['probabilities'], raw['probabilities'])
        self.assertEqual(answer['confidence'], raw['confidence'])
        self.assertEqual(answer['original_result'], raw)
        self.assertEqual(out['model_profile'], 'jev-omni')
        self.assertEqual(out['provider'], 'local-provider')

    def test_jev_omni_rejects_mismatched_index_order_batch_and_native_256(self):
        for n in (2, 255, 256):
            labels = [f'option{i}' for i in range(n)]
            original = {'questions': {'q': {'type': 'choice', 'instructions': 'Choose',
                                           'criteria': dict.fromkeys(labels, None)}}}
            raw = {'prediction': labels[0], 'prediction_index': 0, 'confidence': 1,
                   'probabilities': {label: int(i == 0) for i, label in enumerate(labels)}}
            if n == 256:
                with self.assertRaisesRegex(ValueError, '255'): self.convert(original, raw, 'jev-omni')
                continue
            self.assertEqual(len(self.convert(original, raw, 'jev-omni')['response']['answers']['q']['probabilities']), n)
            for changed in ({**raw, 'prediction_index': True}, {**raw, 'prediction_index': 1},
                            {**raw, 'prediction': 'missing'},
                            {**raw, 'probabilities': dict(reversed(list(raw['probabilities'].items())))},
                            {**raw, 'probabilities': [1] * n}):
                with self.assertRaises(ValueError): self.convert(original, changed, 'jev-omni')
            original['questions']['other'] = original['questions']['q']
            with self.assertRaisesRegex(ValueError, 'one question'): self.convert(original, raw, 'jev-omni')

    def test_new_envelope_media_fields_stay_under_capture_state_control(self):
        original = {'questions': {'q': {'type': 'noul', 'instructions': 'Check'}}}
        media = {'input': ['synthetic native input'], 'assets': {'asset': {'format': 'png', 'data_base64': 'synthetic'}},
                 'media': [{'type': 'image', 'data': 'synthetic'}], 'videos': ['synthetic']}
        out = self.convert(original, {'answers': {'q': {'noul': .7}}, **media}, 'autotrust',
                           model_profile='another-profile')
        for key, value in media.items():
            self.assertEqual(out['response'][key], value)
            self.assertNotIn(key, out['response']['original_metadata'])
        self.assertEqual(out['model_profile'], 'another-profile')

    def test_jev_omni_bare_result_inputs_do_not_duplicate_into_raw_answer(self):
        original = {'questions': {'q': {'type': 'choice', 'instructions': 'Choose', 'criteria': {'a': 'A', 'b': 'B'}}}}
        inputs = {key: ['synthetic input'] for key in capture.INPUT_FIELDS}
        raw = {'prediction': 'a', 'prediction_index': 0, 'confidence': .8,
               'probabilities': {'a': .8, 'b': .2}, 'extension': {'input': 'semantic extension'}, **inputs}
        out = self.convert(original, raw, 'jev-omni')['response']
        for key, value in inputs.items():
            self.assertEqual(out[key], value)
            self.assertNotIn(key, out['original_metadata'])
            self.assertNotIn(key, out['answers']['q']['original_result'])
        self.assertEqual(out['answers']['q']['original_result']['extension'], raw['extension'])

    def test_flat_semif_and_autotrust_inputs_do_not_duplicate_into_raw_answer(self):
        original = {'questions': {'q': {'type': 'choice', 'instructions': 'Choose',
                                       'criteria': {'a': 'A', 'b': 'B'}}}}
        inputs = {key: ['synthetic envelope input'] for key in capture.INPUT_FIELDS}
        raw = {'id': 'q', 'option_ids': ['a', 'b'], 'probabilities': [.2, .8], 'choice': 'b',
               'extension': {'input': 'semantic result extension'}, **inputs}
        before = json.dumps(raw)
        for dialect in ('semif', 'autotrust'):
            with self.subTest(dialect=dialect):
                out = self.convert(original, raw, dialect)['response']
                for key, value in inputs.items():
                    self.assertEqual(out[key], value)
                    self.assertNotIn(key, out['original_metadata'])
                    self.assertNotIn(key, out['answers']['q']['original_result'])
                self.assertEqual(out['answers']['q']['original_result']['extension'], raw['extension'])
                self.assertEqual(out['answers']['q']['probabilities'], {'a': .2, 'b': .8})
                self.assertEqual(out['answers']['q']['choice'], 'b')
                self.assertEqual(json.dumps(raw), before)
                # A named answer's extension fields are not root snapshots.
                nested = self.convert(original, {'answers': {'q': raw}}, dialect)['response']
                self.assertEqual(nested['answers']['q']['original_result'], raw)

    def test_converter_keeps_input_fields_in_the_capture_envelope(self):
        original = {'questions': {'check': {'type': 'noul', 'instructions': 'Check'}}}
        inputs = {'state': 'private text', 'images': ['private images'],
                  'image': 'private image', 'audio': 'private audio', 'video': 'private video'}
        raw = {'answers': {'check': {'p_true': .7}}, 'usage': {'questions': 1},
               'runtime': {'state': 'ready'}, **inputs}
        out = capture.convert(original, raw, dialect='autotrust', event_id='event-1',
                              timestamp=1, source='app', provider='autotrust')
        for field, value in inputs.items():
            self.assertEqual(out['response'][field], value)
            self.assertNotIn(field, out['response']['original_metadata'])
        self.assertEqual(out['response']['original_metadata']['usage'], {'questions': 1})
        self.assertEqual(out['response']['original_metadata']['runtime'], {'state': 'ready'})

    def test_nanojev_keeps_probability_instead_of_thresholded_boolean(self):
        raw = {'states': [{'id': 'one', 'answers': {
            'route': {'type': 'choice', 'choice': 'b', 'probabilities': {'a': .2, 'b': .8}},
            'check': {'type': 'boolean', 'value': True, 'p_true': .6},
            'rating': {'type': 'score', 'score': .7, 'probabilities': {'0': .3, '1': .7}}}}]}
        out = capture.map_answers(request(), raw, 'nanojev')
        self.assertEqual(out['check']['noul'], .6)
        self.assertNotIn('confidence', out['route'])
        self.assertNotIn('legend', out['rating'])
        self.assertEqual(out['route']['original_result'], raw['states'][0]['answers']['route'])
        raw['states'].append({'id': 'two', 'answers': {}})
        with self.assertRaises(ValueError): capture.map_answers(request(), raw, 'nanojev')
        self.assertEqual(out, capture.map_answers(request(), raw, 'nanojev', 'one'))

    def test_agentjev_matches_ids_and_does_not_relabel_top_probability_as_confidence(self):
        raw = {'results': [{'id': 'one', 'answers': [
            {'id': 'check', 'type': 'boolean', 'value': True, 'probability': .7, 'distribution': {'true': .7, 'false': .3}},
            {'id': 'rating', 'type': 'score', 'score': .8, 'legend': ['Low', 'High'], 'distribution': {'0': .2, '1': .8}},
            {'id': 'route', 'type': 'choice', 'value': 'b', 'top_probability': .8, 'distribution': {'a': .2, 'b': .8}}]}]}
        out = capture.map_answers(request(), raw, 'agentjev')
        self.assertEqual(out['route']['choice'], 'b')
        self.assertNotIn('confidence', out['route'])
        self.assertEqual(out['rating']['legend'], {'0': 'Low', '1': 'High'})
        raw['results'][0]['answers'].append(raw['results'][0]['answers'][0])
        with self.assertRaises(ValueError): capture.map_answers(request(), raw, 'agentjev')

    def test_anyjev_rejects_numeric_bin_score_without_explicit_ordinal_mapping(self):
        raw = {'questions': {'route': {'kind': 'choice', 'answer': 'b', 'confidence': .8, 'distribution': {'a': .2, 'b': .8}},
                             'check': {'kind': 'noul', 'answer': True, 'probability': .7},
                             'rating': {'kind': 'score', 'value': 7.5, 'distribution': {'5': .5, '10': .5}}}}
        with self.assertRaisesRegex(ValueError, 'ordinal'): capture.map_answers(request(), raw, 'anyjev')
        original = request(); del original['questions']['rating']; del raw['questions']['rating']
        self.assertEqual(capture.map_answers(original, raw, 'anyjev')['check']['noul'], .7)

    def test_autotrust_arrays_require_explicit_option_correspondence(self):
        original = request(); original['questions'] = {'route': original['questions']['route']}
        raw = {'options': ['a', 'b'], 'probabilities': [.2, .8], 'choice': 'b'}
        self.assertEqual(capture.map_answers(original, raw, 'autotrust')['route']['probabilities'], {'a': .2, 'b': .8})
        raw['options'] = ['a']
        with self.assertRaisesRegex(ValueError, 'equally sized'): capture.map_answers(original, raw, 'autotrust')

    def test_semif_full_distribution_argmax_preserves_uncalibrated_status(self):
        original = request(); original['questions'] = {'route': original['questions']['route']}
        raw = {'id': 'route', 'option_ids': ['a', 'b'], 'probabilities': [.2, .8], 'probability_status': 'uncalibrated'}
        out = capture.map_answers(original, raw, 'semif')['route']
        self.assertEqual(out['choice'], 'b')
        self.assertEqual(out['original_result']['probability_status'], 'uncalibrated')
        self.assertIn('mapping_note', out)
        raw['id'] = 'other'
        with self.assertRaises(ValueError): capture.map_answers(original, raw, 'semif')

    def test_semif_derived_choice_requires_complete_numeric_probabilities(self):
        original = {'questions': {'route': {'type': 'choice', 'criteria': {'a': 'A', 'b': 'B'}}}}
        for distribution in ({'a': 1}, {'a': .2, 'b': .3, 'other': .5}, {},
                             {'a': '0.2', 'b': '0.8'}, {'a': False, 'b': True},
                             {'a': -.1, 'b': 1.1}, {'a': float('nan'), 'b': .8},
                             {'a': .2, 'b': float('inf')}):
            with self.subTest(distribution=distribution):
                with self.assertRaisesRegex(ValueError, 'SemIf argmax requires'):
                    capture.map_answers(original, {'id': 'route', 'probabilities': distribution}, 'semif')
        # Named and array distributions preserve correspondence, including
        # reordered IDs. Ties retain the existing first-reported winner.
        for raw, expected in (({'probabilities': {'b': .8, 'a': .2}}, 'b'),
                              ({'option_ids': ['b', 'a'], 'probabilities': [.8, .2]}, 'b'),
                              ({'probabilities': {'b': .5, 'a': .5}}, 'b'),
                              ({'probabilities': {'a': 0, 'b': 1}}, 'b'),
                              ({'probabilities': {'a': 1, 'b': 0}}, 'a')):
            self.assertEqual(capture.map_answers(original, raw, 'semif')['route']['choice'], expected)
        for criteria in (None, {}, [], 'a,b'):
            with self.subTest(criteria=criteria):
                malformed = {'questions': {'route': {'type': 'choice', 'criteria': criteria}}}
                with self.assertRaisesRegex(ValueError, 'SemIf argmax requires'):
                    capture.map_answers(malformed, {'probabilities': {'a': .2, 'b': .8}}, 'semif')
        # Explicit provider answers remain inspectable, even if malformed;
        # Observer's downstream validation still owns their validity.
        raw = {'choice': 'a', 'probabilities': {'a': .2}}
        answer = capture.map_answers(original, raw, 'semif')['route']
        self.assertEqual(answer['choice'], 'a')
        self.assertEqual(answer['original_result'], raw)
        self.assertNotIn('mapping_note', answer)

    def test_converter_does_not_turn_custom_usage_counts_into_billed_tokens(self):
        original = request(); original['questions'] = {'route': original['questions']['route']}
        raw = {'answers': {'route': {'choice': 'a', 'distribution': {'a': .8, 'b': .2}}}, 'usage': {'questions': 1}}
        out = capture.convert(original, raw, dialect='autotrust', event_id='event-1', timestamp=1, source='app', provider='autotrust')
        self.assertNotIn('usage', out['response'])
        self.assertNotIn('model', out['response'])
        self.assertIsNone(out['duration_ms'])
        self.assertEqual(out['response']['original_metadata']['usage'], {'questions': 1})


class CaptureCliTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='observer-capture-cli-')
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.request_path = self.directory / 'request.json'
        self.response_path = self.directory / 'response.json'
        self.output_path = self.directory / 'capture.jsonl'
        self.write_inputs()

    def write_inputs(self, original=None, response=None, ensure_ascii=True):
        original = request() if original is None else original
        response = {'answers': {}} if response is None else response
        self.request_path.write_text(json.dumps(original, ensure_ascii=ensure_ascii), encoding='utf-8')
        self.response_path.write_text(json.dumps(response, ensure_ascii=ensure_ascii), encoding='utf-8')

    def run_cli(self, *extra):
        environment = os.environ.copy()
        environment.update({'LC_ALL': 'C', 'PYTHONUTF8': '0', 'PYTHONCOERCECLOCALE': '0'})
        return subprocess.run([
            sys.executable, str(Path(__file__).with_name('capture-decision.py')),
            '--request', str(self.request_path), '--response', str(self.response_path),
            '--id', 'event-1', '--timestamp', '1', '--source', 'app', '--provider', 'typesafe',
            '--output', str(self.output_path), *extra,
        ], env=environment, capture_output=True, timeout=10)

    def test_utf8_inputs_and_output_do_not_depend_on_locale(self):
        original = request()
        original['questions']['route']['instructions'] = '분류 😀'
        response = {'answers': {}, 'note': '응답 😀'}
        for ensure_ascii in (False, True):
            with self.subTest(escaped_input=ensure_ascii):
                self.output_path = self.directory / f'capture-{ensure_ascii}.jsonl'
                self.write_inputs(original, response, ensure_ascii=ensure_ascii)
                result = self.run_cli()
                self.assertEqual(result.returncode, 0, result.stderr)
                payload = self.output_path.read_bytes()
                self.assertIn('분류 😀'.encode('utf-8'), payload)
                decoded = json.loads(payload.decode('utf-8'))
                self.assertEqual(decoded['request'], original)
                self.assertEqual(decoded['response'], response)
                self.assertTrue(payload.endswith(b'\n'))
                self.output_path.unlink()

    def test_failed_serialization_or_encoding_does_not_reserve_output(self):
        invalid_unicode = request()
        invalid_unicode['state'] = '\ud800'
        cases = ((request(), ('--duration-ms', 'NaN')), (invalid_unicode, ()))
        for index, (original, extra) in enumerate(cases):
            with self.subTest(extra=extra):
                self.output_path = self.directory / f'capture-{index}.jsonl'
                self.write_inputs(original)
                result = self.run_cli(*extra)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b'Capture conversion failed:', result.stderr)
                self.assertFalse(self.output_path.exists())
                self.write_inputs()
                corrected = self.run_cli('--duration-ms', '10')
                self.assertEqual(corrected.returncode, 0, corrected.stderr)
                self.output_path.unlink()

    def test_existing_output_is_preserved(self):
        previous = b'previous capture\n'
        self.output_path.write_bytes(previous)
        result = self.run_cli()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.output_path.read_bytes(), previous)

    def test_incomplete_semif_distribution_fails_before_creating_output(self):
        original = {'questions': {'route': {'type': 'choice', 'criteria': {'a': 'A', 'b': 'B'}}}}
        self.write_inputs(original, {'id': 'route', 'option_ids': ['a'], 'probabilities': [1]})
        result = self.run_cli('--dialect', 'semif')
        self.assertEqual(result.returncode, 1)
        self.assertIn(b'SemIf argmax requires', result.stderr)
        self.assertNotIn(b'Traceback', result.stderr)
        self.assertFalse(self.output_path.exists())
        self.write_inputs(original, {'id': 'route', 'option_ids': ['a', 'b'], 'probabilities': [.2, .8]})
        result = self.run_cli('--dialect', 'semif')
        self.assertEqual(result.returncode, 0, result.stderr)
        record = json.loads(self.output_path.read_text(encoding='utf-8'))
        self.assertEqual(record['response']['answers']['route']['choice'], 'b')

    def test_malformed_roots_and_answers_fail_cleanly_without_reserving_output(self):
        valid_request = {'questions': {'q': {'type': 'noul', 'instructions': 'Check'}}}
        valid_response = {'answers': {'q': {'noul': .7}}}
        for original, response in (([], valid_response), (valid_request, []),
                                   ({'questions': {'q': None}}, valid_response),
                                   (valid_request, {'answers': {'q': None}}),
                                   (valid_request, {'answers': [None]})):
            self.write_inputs(original, response)
            result = self.run_cli('--dialect', 'autotrust')
            self.assertEqual(result.returncode, 1)
            self.assertIn(b'Capture conversion failed:', result.stderr)
            self.assertNotIn(b'Traceback', result.stderr)
            self.assertFalse(self.output_path.exists())
        self.write_inputs(valid_request, valid_response)
        self.assertEqual(self.run_cli('--dialect', 'autotrust').returncode, 0)

    def test_duplicate_json_option_keys_are_rejected_before_correspondence_is_lost(self):
        self.request_path.write_text('{"questions":{"q":{"type":"choice","instructions":"Choose",'
                                     '"criteria":{"a":"first","a":"second"}}}}', encoding='utf-8')
        result = self.run_cli('--dialect', 'jev-omni')
        self.assertEqual(result.returncode, 1)
        self.assertIn(b'Duplicate JSON object keys', result.stderr)
        self.assertNotIn(b'Traceback', result.stderr)
        self.assertFalse(self.output_path.exists())
        self.write_inputs()
        self.response_path.write_text('{"answers":{},"answers":{}}', encoding='utf-8')
        result = self.run_cli()
        self.assertEqual(result.returncode, 1)
        self.assertFalse(self.output_path.exists())


if __name__ == '__main__': unittest.main()
