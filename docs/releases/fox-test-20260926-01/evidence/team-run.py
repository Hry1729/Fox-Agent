"""Bounded, serial build/test runner for the isolated F1 worktree."""
import os, sys, subprocess, time, json, hashlib
from pathlib import Path
root=Path(__file__).resolve().parent/'team-evidence'
root.mkdir(exist_ok=True)
repo=Path(os.environ.get('F1_REVIEW_WORKTREE','D:/python/projects/Fox/worktrees/f1-r1-20260924'))
name=sys.argv[1]
seconds=int(sys.argv[2])
args=sys.argv[3:]
env=os.environ.copy()
env['CARGO_TARGET_DIR']='D:/python/projects/Fox/Fox/.test-target'
native_dirs=[]
if os.name=='nt':
    # Match the native runtime search path Cargo supplies to test executables.
    # Only use real build outputs; never modify the machine PATH or fake a DLL.
    import ctypes
    ctypes.windll.kernel32.SetErrorMode(0x0001 | 0x0002 | 0x8000)
    target=Path(env['CARGO_TARGET_DIR'])
    native_dirs=[str(p.parent) for p in (target/'debug/build').glob('zvec-rust-sys-*/out/zvec-prebuilt/zvec_c_api.dll')]
    env['PATH']=os.pathsep.join(native_dirs+[str(target/'debug/deps'),env.get('PATH','')])
    if Path(args[0]).name.startswith('fox_desktop_lib-') and not native_dirs:
        raise SystemExit('Real zvec_c_api.dll build output missing; run cargo test to prepare native dependencies.')
env['FOX_DATA_DIR']=str(root/'data'/name)
# Native Windows libraries can still enforce MAX_PATH. Keep synthetic scratch
# roots short; the durable logs remain beside this script.
tmp=Path('D:/python/projects/Fox/buildtmp/review-tmp')/hashlib.sha256(name.encode()).hexdigest()[:10]
tmp.mkdir(parents=True,exist_ok=True)
env['TMP']=env['TEMP']=str(tmp)
started=time.monotonic()
source_head=subprocess.run(['git','rev-parse','HEAD'],cwd=repo,capture_output=True,text=True,timeout=10).stdout.strip()
source_status=subprocess.run(['git','status','--porcelain'],cwd=repo,capture_output=True,text=True,timeout=10).stdout
with (root/(name+'.log')).open('wb') as out:
    p=subprocess.Popen(args,cwd=repo,env=env,stdout=out,stderr=subprocess.STDOUT)
    (root/(name+'.pid.json')).write_text(json.dumps(dict(pid=p.pid,command=args)),encoding='utf-8')
    try: code=p.wait(seconds)
    except subprocess.TimeoutExpired:
        subprocess.run(['taskkill','/PID',str(p.pid),'/T','/F'],stdout=out,stderr=out,timeout=15)
        # The inherited process handle can still terminate our own child when
        # a separately started taskkill cannot open it under the sandbox ACL.
        if p.poll() is None:
            p.kill()
            p.wait(timeout=10)
        code=124
record=dict(command=args,cwd=str(repo),sourceHead=source_head,sourceStatus=source_status,nativeLibraryDirectories=native_dirs,timeoutSeconds=seconds,exitCode=code,seconds=round(time.monotonic()-started,2))
if Path(args[0]).is_file():
    with Path(args[0]).open('rb') as binary:
        record['executableSha256'] = hashlib.file_digest(binary, 'sha256').hexdigest()
record['rustTestThreadsEnvironment'] = env.get('RUST_TEST_THREADS')
record['libsqlite3FlagsEnvironment'] = env.get('LIBSQLITE3_FLAGS')
record['temporaryDirectory'] = str(tmp)
(root/(name+'.json')).write_text(json.dumps(record,indent=2),encoding='utf-8')
print(json.dumps(record))
sys.exit(code)
