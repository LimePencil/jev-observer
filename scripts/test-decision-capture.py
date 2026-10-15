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


if __name__ == '__main__': unittest.main()
