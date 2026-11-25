#!/usr/bin/env python3
"""Opt-in real-model harness evaluation. Requires Python 3.11+, never packages credentials."""
import argparse, json, os, signal, subprocess, tempfile, time, tomllib
from pathlib import Path

def run(binary, config, out, trials):
    model = tomllib.loads(config.read_text()).get('model', {})
    results = []
    for trial in range(trials):
        for case in ('locate', 'edit', 'command'):
            with tempfile.TemporaryDirectory(prefix='colibri-agent-eval-') as directory:
                root = Path(directory); work = root/'work'; work.mkdir()
                private = root/'config.toml'
                fields = '\n'.join(k+' = '+json.dumps(v) for k,v in model.items() if isinstance(v,(str,int,float,bool)))
                private.write_text('[model]\n'+fields+'\n[tools]\nenabled=["files","shell"]\nmax_shell_seconds=15\nmax_result_chars=4000\n[memory]\nenabled=false\n[session]\ntranscript=false\nrestore_transcript=false\n')
                private.chmod(0o600)
                if case == 'locate':
                    path=work/'events.log'; path.write_text(('ordinary event payload '+'x'*80+'\n')*80000+'TARGET_RESULT=quartz-917\n')
                    prompt=f'Find TARGET_RESULT in {path}. Return its exact value. Do not modify files.'
                    check=lambda text:'quartz-917' in text
                elif case == 'edit':
                    path=work/'settings.txt'; before='setting = unchanged\n'*2000+'retry_delay = 4\n';path.write_text(before)
                    prompt=f'Change only retry_delay from 4 to 7 in {path}, verify the change, and report the result. Preserve every other byte.'
                    check=lambda text:path.read_text()==before.replace('retry_delay = 4','retry_delay = 7')
                else:
                    script=work/'check.sh'; target=work/'result.txt';script.write_text('sleep 2\nprintf "verified-output" > '+str(target)+'\n')
                    prompt=f'Run sh {script}, wait for completion, then read {target} and report its exact content. Do not claim completion before checking the result.'
                    check=lambda text:target.is_file() and target.read_text()=='verified-output' and 'verified-output' in text
                started=time.monotonic();p=subprocess.Popen([str(binary),'--config',str(private),'ask',prompt],cwd=work,env=dict(os.environ,COLIBRI_HOME=str(root/'state')),stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,start_new_session=True)
                timed_out=False
                try: stdout,stderr=p.communicate(timeout=90)
                except subprocess.TimeoutExpired:
                    timed_out=True;os.killpg(p.pid,signal.SIGKILL);stdout,stderr=p.communicate()
                passed=not timed_out and p.returncode==0 and check(stdout)
                item={'case':case,'trial':trial+1,'passed':passed,'elapsed_seconds':round(time.monotonic()-started,3),'exit_code':p.returncode,'timed_out':timed_out,'response_chars':len(stdout)}
                results.append(item);print(case, 'PASS' if passed else 'FAIL', item['elapsed_seconds'],flush=True)
    report={'model':model.get('model'),'binary':str(binary),'trials':trials,'results':results,'note':'Small real-model smoke evaluation; not a statistically reliable success-rate estimate.'}
    out.parent.mkdir(parents=True,exist_ok=True);out.write_text(json.dumps(report,indent=2))

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--config',type=Path,default=Path.home()/'.colibri/config.toml');p.add_argument('--out',type=Path,required=True);p.add_argument('--trials',type=int,default=1)
    a=p.parse_args();assert 1<=a.trials<=10;run(a.binary.resolve(),a.config,a.out,a.trials)
