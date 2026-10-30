"""Exercise release failure handling with synthetic artifacts and fake host commands."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

class DeploymentTests(unittest.TestCase):
    def test_backend_readiness_failure_restores_previous_release(self):
        self.exercise_backend(False)

    def test_backend_success_reports_tested_sha(self):
        self.exercise_backend(True)

    def test_backend_restart_failure_restores_previous_release(self):
        self.exercise_backend(True, fail_restart=True)

    def exercise_backend(self, healthy, fail_restart=False):
        bash = shutil.which("bash")
        if os.name == "nt":
            bash = r"C:\\Program Files\\Git\\bin\\bash.exe"
        self.assertTrue(bash and Path(bash).exists(), "Bash is required for deployment tests")
        with tempfile.TemporaryDirectory(prefix="ecg-deploy-test-") as temporary:
            directory = Path(temporary).resolve()
            root = directory / "service"
            old_sha = "a" * 40
            sha = "b" * 40
            previous = root / "releases" / old_sha
            previous.mkdir(parents=True)
            package = directory / "package"
            package.mkdir()
            (package / "ecg-backend").write_text("synthetic binary\n", newline="\n")
            (package / "RELEASE_SHA").write_text(sha + "\n", newline="\n")
            (package / "SHA256SUMS").write_text("".join(
                hashlib.sha256((package / filename).read_bytes()).hexdigest() + "  " + filename + "\n"
                for filename in ["ecg-backend", "RELEASE_SHA"]
            ), newline="\n")
            artifact = directory / "backend.tar.gz"
            with tarfile.open(artifact, "w:gz") as archive:
                for file in package.iterdir():
                    archive.add(file, arcname=file.name)
            def shell_path(path):
                if os.name == "nt":
                    return subprocess.check_output([bash, "-c", 'cygpath -u "$1"', "_", str(path)], text=True).strip()
                return str(path)
            environment = dict(os.environ)
            if os.name == "nt": environment["MSYS"]="winsymlinks:lnk"
            shell_root = shell_path(root)
            subprocess.run([bash, "-c", 'ln -s "$1" "$2"', "_", shell_path(previous), shell_path(root / "current")], check=True, env=environment)
            fake_bin = directory / "bin"
            fake_bin.mkdir()
            for name, body in {
                "sudo": "exit 0\n",
                "sleep": "exit 0\n",
                "curl": "printf '%s\\n' " + ("'{\"release\":\"" + sha + "\"}'" if healthy else "'{\"release\":\"wrong\"}'") + "\n",
            }.items():
                file = fake_bin / name
                file.write_text("#!/usr/bin/env bash\n" + body, newline="\n")
                file.chmod(0o755)
            if os.name == "nt":
                # Git Bash lacks util-linux flock; Linux CI exercises the real primitive.
                (fake_bin / "flock").write_text("#!/usr/bin/env bash\nexit 0\n", newline="\n")
                (fake_bin / "python3").write_text("#!/usr/bin/env bash\nexec '" + shell_path(Path(sys.executable)) + "' \"$@\"\n", newline="\n")
            if fail_restart:
                (fake_bin / "sudo").write_text("#!/usr/bin/env bash\nif [[ ! -f \"$RESTART_MARKER\" ]]; then touch \"$RESTART_MARKER\"; exit 1; fi\nexit 0\n", newline="\n")
                environment["RESTART_MARKER"]=shell_path(directory / "restart-marker")
            source = (Path(__file__).resolve().parents[1] / "deploy" / "release.sh").read_text()
            script = directory / "release.sh"
            script.write_text(source.replace("root=/opt/ecgrhythmia", "root=" + shell_root), newline="\n")
            # Bash converts native Windows paths; force an explicit PATH inside its shell.
            command = 'export PATH="$1:$PATH"; bash "$2" "$3" "$4"'
            result = subprocess.run([bash, "-c", command, "_", shell_path(fake_bin), shell_path(script), shell_path(artifact), sha],
                                    capture_output=True, text=True, env=environment)
            self.assertEqual(result.returncode, 0 if healthy and not fail_restart else 1, result.stdout + result.stderr)
            target = subprocess.check_output([bash, "-c", 'readlink -f "$1"', "_", shell_path(root / "current")], text=True, env=environment).strip()
            self.assertEqual(target, shell_path(root / "releases" / (sha if healthy and not fail_restart else old_sha)))
            if not healthy or fail_restart:
                self.assertIn("Readiness failed", result.stderr)

if __name__ == "__main__":
    unittest.main()
