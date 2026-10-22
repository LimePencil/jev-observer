#!/usr/bin/env python3
"""Offline catalog metadata/provenance checks; no provider or model execution."""
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
CATALOG = json.loads((ROOT / 'src/model_catalog.json').read_text())
MODELS = {model['id']: model for model in CATALOG['models']}


class CatalogTests(unittest.TestCase):
    def test_identities_are_unique_and_transport_is_explicit(self):
        self.assertEqual(len(MODELS), len(CATALOG['models']))
        for model in MODELS.values():
            with self.subTest(model=model['id']):
                self.assertEqual(model['model_profile'], model['id'])
                self.assertTrue(model['provider'])
                self.assertIn(model['protocol'], ('systemone', 'openai-decisions', 'cloudflare-systemone'))
                self.assertIn(model['integration'], ('proxy', 'capture'))
                self.assertEqual(model['upstream'] is None, model['integration'] == 'capture')
                self.assertRegex(model['checked_at'], r'^2026-10-(03|23)$')

    def test_old_snapshot_dates_are_not_relabelled_as_current_research(self):
        old = {'milliseconds', 'openrouter', 'vercel', 'laya', 'kev', 'decider', 'nimble', 'von',
               'openjev', 'codiv', 'open-jev', 'reflex', 'jevk5', 'openthai', 'jeff', 'rizzo',
               'ollaya', 'edgejev', 'openjev-sglang', 'semif', 'tev1', 'nanojev', 'agentjev',
               'anyjev', 'verdict', 'valen', 'eikos', 'dev', 'blink', 'autotrust', 'neohorse',
               'matilda', 'jev-style', 'intern-decision', 'frida', 'vllm-sr'}
        self.assertEqual({id for id, model in MODELS.items() if model['checked_at'] == '2026-10-03'}, old)

    def test_new_and_rereviewed_entries_have_pinned_primary_sources(self):
        manifest = json.loads((ROOT / 'research/models/2026-10-23-sources.json').read_text())
        sources = {source['url']: source for source in manifest['sources']}
        for model in MODELS.values():
            if model['checked_at'] == '2026-10-03':
                continue
            with self.subTest(model=model['id']):
                self.assertTrue(model['source_pins'])
                for source in model['source_pins']:
                    self.assertEqual(source, sources[source['url']])
                    self.assertTrue(re.fullmatch(r'[a-f0-9]{64}', source.get('sha256', '')) or
                                    re.fullmatch(r'[a-f0-9]{40}', source.get('git_blob', '')))
                    if source['revision'] is not None:
                        self.assertIn(source['revision'], source['url'])

    def test_transport_provider_and_validation_profile_are_independent(self):
        self.assertEqual(MODELS['cloudflare-clef']['provider'], 'cloudflare')
        self.assertEqual(MODELS['cloudflare-clef']['protocol'], 'cloudflare-systemone')
        self.assertEqual(MODELS['ollama-clef']['provider'], 'ollama')
        self.assertEqual(MODELS['ollama-clef']['profile']['choice_max'], 26)
        self.assertNotIn('probability_decimals', MODELS['ollama-clef']['profile'])
        self.assertEqual(MODELS['clef']['profile']['probability_decimals'], 4)
        self.assertEqual(MODELS['openai-decisions']['protocol'], 'openai-decisions')
        self.assertEqual(MODELS['openai-decisions']['profile'], {})

    def test_distinct_omni_contracts_and_presence_flags_are_not_pooled(self):
        self.assertEqual(MODELS['onejev']['profile']['probability_decimals'], 6)
        self.assertTrue(MODELS['onejev']['profile']['nullable_score_levels'])
        self.assertEqual(MODELS['omnijev']['profile']['probability_decimals'], 4)
        self.assertTrue(MODELS['omnijev']['profile']['reported_abstention'])
        self.assertTrue(MODELS['omnijev']['profile']['modal_score'])
        self.assertEqual(MODELS['jev-omni']['profile']['choice_min'], 2)
        self.assertTrue(MODELS['typesafe']['profile']['box_answers'])
        self.assertNotIn('box_presence', MODELS['typesafe']['profile'])
        self.assertTrue(MODELS['h2o-lightning']['profile']['box_presence'])


if __name__ == '__main__':
    unittest.main()
