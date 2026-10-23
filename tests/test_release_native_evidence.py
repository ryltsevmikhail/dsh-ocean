"""Bind reported native ELF and notice bytes to the actual release archive."""
import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

class NativeEvidence(unittest.TestCase):
    def test_archive_binding_rejects_different_elf_and_notice_bytes(self):
        self.assertTrue((ROOT/'scripts/release-native-evidence.py').is_file(), 'release archive binding is not implemented')
        spec=importlib.util.spec_from_file_location('release_native',ROOT/'scripts/release-native-evidence.py')
        module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory(prefix='ocean-native-evidence-') as tmp:
            root=Path(tmp);binary=root/'dsh-ocean';binary.write_bytes(b'owned-built-elf')
            notice=root/'notice';notice.write_bytes(b'copyright exact')
            def archive(elf,copy):
                path=root/'release.tar.gz'
                with tarfile.open(path,'w:gz') as tar:
                    for name,data in [('pkg/dsh-ocean',elf),('pkg/LICENSE',copy)]:
                        info=tarfile.TarInfo(name);info.size=len(data);tar.addfile(info,io.BytesIO(data))
                return path
            inputs={'LICENSE':notice}
            good=module.verify_archive(archive(binary.read_bytes(),notice.read_bytes()),binary,inputs)
            self.assertEqual(good['binarySha256'],hashlib.sha256(binary.read_bytes()).hexdigest())
            self.assertEqual(good['memberSha256']['LICENSE'],hashlib.sha256(notice.read_bytes()).hexdigest())
            with self.assertRaisesRegex(ValueError,'ELF'):
                module.verify_archive(archive(b'stale-or-rebuilt',notice.read_bytes()),binary,inputs)
            with self.assertRaisesRegex(ValueError,'payload'):
                module.verify_archive(archive(binary.read_bytes(),b'truncated'),binary,inputs)

    def test_dpkg_diversion_headers_are_not_package_owners(self):
        from unittest.mock import patch
        from types import SimpleNamespace
        spec=importlib.util.spec_from_file_location('release_native',ROOT/'scripts/release-native-evidence.py')
        module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
        output='diversion by libc6 from: /lib/ld-linux.so\ndiversion by libc6 to: /lib/ld-linux.so.usr-is-merged\nlibc6:arm64: /lib/ld-linux.so\n'
        def query(args):
            self.assertEqual(args[-1],'libc6:arm64')
            return 'libc6\t2.39-test\tarm64\tglibc\t2.39-test\n'
        with patch.object(module.subprocess,'run',return_value=SimpleNamespace(returncode=0,stdout=output)),patch.object(module,'command',side_effect=query):
            record=module.package_origin(Path('/lib/ld-linux.so'))
        self.assertEqual(record['package'],'libc6')
        self.assertEqual(record['sourcePackage'],'glibc')

    def test_map_output_must_identify_this_exact_cargo_elf(self):
        spec=importlib.util.spec_from_file_location('release_native',ROOT/'scripts/release-native-evidence.py')
        module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
        self.assertTrue(hasattr(module,'verify_map_output'))
        with tempfile.TemporaryDirectory(prefix='ocean-map-output-') as tmp:
            release=Path(tmp)/'release';(release/'deps').mkdir(parents=True)
            binary=release/'dsh-ocean';binary.write_bytes(b'current-elf')
            output=release/'deps/dsh_ocean-test';output.write_bytes(binary.read_bytes())
            module.verify_map_output(f'OUTPUT({output} elf64-x86-64)',binary)
            output.write_bytes(b'stale-elf')
            with self.assertRaisesRegex(ValueError,'map output'):
                module.verify_map_output(f'OUTPUT({output} elf64-x86-64)',binary)
            with self.assertRaisesRegex(ValueError,'map output'):
                module.verify_map_output('unrelated dsh_ocean name only',binary)

    def test_gnu_and_lld_map_references_are_not_reported_as_selected_code(self):
        spec=importlib.util.spec_from_file_location('release_native',ROOT/'scripts/release-native-evidence.py')
        module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
        self.assertTrue(hasattr(module,'map_references'), 'map inventory must distinguish references from membership')
        gnu='Archive member included to satisfy reference by file (symbol)\n/build/used.a(member.o)\nDiscarded input sections\n/build/discarded.a(old.o)\nLOAD /build/unused.a\nOUTPUT(/build/dsh_ocean-test elf64-x86-64)\n'
        report=module.map_references(gnu)
        self.assertEqual(report['format'],'gnu-ld')
        self.assertEqual({r['path'] for r in report['references']},{'/build/used.a','/build/discarded.a','/build/unused.a'})
        self.assertFalse(report['provesLinkedMembership'])
        lld='VMA LMA Size Align Out In Symbol\n1000 1000 10 16 /build/used.rlib(member.o):(.text)\n'
        report=module.map_references(lld)
        self.assertEqual(report['format'],'lld')
        self.assertEqual(report['references'][0]['path'],'/build/used.rlib')
        self.assertFalse(report['provesLinkedMembership'])

if __name__=='__main__':unittest.main()
