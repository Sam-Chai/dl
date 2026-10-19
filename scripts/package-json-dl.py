"""Package one already-built binary; never claims to build or validate a target."""
import argparse
import hashlib
import json
from pathlib import Path
import tarfile
import tomllib
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument('--target', required=True)
parser.add_argument('--binary', required=True, type=Path)
parser.add_argument('--output', default=Path('dist'), type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
version = tomllib.loads((root / 'json-dl/Cargo.toml').read_text())['package']['version']
args.output.mkdir(parents=True, exist_ok=True)
name = f'json-dl-v{version}-{args.target}'
exe = 'json-dl.exe' if 'windows' in args.target else 'json-dl'
files = [(args.binary, exe), (root / 'LICENSE', 'LICENSE'),
         (root / 'docs/json-dl.md', 'json-dl-protocol.md')]
if 'windows' in args.target:
    archive = args.output / (name + '.zip')
    with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as out:
        for path, dest in files:
            out.write(path, dest)
else:
    archive = args.output / (name + '.tar.gz')
    with tarfile.open(archive, 'w:gz') as out:
        for path, dest in files:
            info = out.gettarinfo(str(path), dest)
            info.uid = info.gid = 0
            info.uname = info.gname = ''
            info.mode = 0o755 if dest == exe else 0o644
            with path.open('rb') as source:
                out.addfile(info, source)

def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()

archive_hash = digest(archive)
metadata = dict(version=version, protocol=1, target=args.target,
                executable=exe, executable_sha256=digest(args.binary),
                archive=archive.name, archive_sha256=archive_hash)
(args.output / (name + '.json')).write_text(json.dumps(metadata, indent=2) + '\n')
(args.output / (archive.name + '.sha256')).write_text(f'{archive_hash}  {archive.name}\n')
print(archive)
