"""Embedded Ocean catalogs: no external language package or runtime download."""
import json
from pathlib import Path
import re
import unittest

ROOT=Path(__file__).resolve().parents[1]
LOCALES=('en','ru','es','pt-BR','de','fr','ja','ko')
KEYS={'common.ok','common.cancel','common.close','language.title','language.system',
      'language.restart_notice','setup.starting','setup.retry','setup.recover','setup.log_lines'}

def unique_object(pairs):
    result={}
    for key,value in pairs:
        if key in result:raise ValueError('duplicate catalog key: '+key)
        result[key]=value
    return result

def placeholders(value):
    fields=set()
    # Match the Rust plain-text grammar, not Python's richer format syntax.
    for token in re.finditer(r'\{\{|\}\}|\{([a-z][a-z0-9_]*)\}|[{}]',value):
        if token.group(1) is not None:
            fields.add(token.group(1))
        elif token.group(0) not in ('{{','}}'):
            raise ValueError('unsupported named placeholder')
    return fields

class NativeLocaleCatalogs(unittest.TestCase):
    def test_all_eight_locales_ship_complete_plain_message_catalogs(self):
        directory=ROOT/'locales/ocean'
        self.assertTrue(directory.is_dir(),'Ocean has no built-in locale catalogs')
        self.assertEqual({p.name for p in directory.iterdir()}, {x+'.json' for x in LOCALES})
        catalogs={code:json.loads((directory/(code+'.json')).read_text(),object_pairs_hook=unique_object) for code in LOCALES}
        self.assertTrue(KEYS.issubset(catalogs['en']))
        for locale,catalog in catalogs.items():
            with self.subTest(locale=locale):
                self.assertEqual(set(catalog),set(catalogs['en']))
                for key,value in catalog.items():
                    self.assertIsInstance(value,str)
                    self.assertTrue(value.strip())
                    self.assertEqual(placeholders(value),placeholders(catalogs['en'][key]))
                self.assertEqual(placeholders(catalog['setup.log_lines']),{'count'})
                if locale!='en':self.assertNotEqual(catalog['language.title'],catalogs['en']['language.title'])

    def test_duplicate_and_unsupported_placeholder_contract_is_strict(self):
        with self.assertRaisesRegex(ValueError,'duplicate'):json.loads('{"x":"a","x":"b"}',object_pairs_hook=unique_object)
        for text in ('{x.y}','{x!r}','{x:>10}','{x:}','{unterminated'):
            with self.subTest(text=text),self.assertRaises(ValueError):placeholders(text)
        self.assertEqual(placeholders('{{literal}} {path}'),{'path'})

if __name__=='__main__':unittest.main()
