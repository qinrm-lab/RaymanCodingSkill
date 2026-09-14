"""Isolated real-process acceptance for the optional SaveStatus global adapter."""
import argparse, contextlib, hashlib, json, os, shutil, sqlite3, subprocess, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


@contextlib.contextmanager
def database(path):
    connection=sqlite3.connect(path)
    try:
        with connection:
            yield connection
    finally:
        connection.close()


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--save-runtime',type=Path,required=True)
    parser.add_argument('--worker',type=Path,required=True)
    parser.add_argument('--client',type=Path,required=True)
    args=parser.parse_args()
    repo=Path(__file__).resolve().parents[1]
    runtime=args.save_runtime.resolve()
    env=os.environ.copy()
    env['SAVE_WORK_STATUS_OUTPUT']='compact'
    def run(argv,expect=0):
        result=subprocess.run([str(x) for x in argv],cwd=repo,env=env,capture_output=True,text=True,encoding='utf-8',timeout=90)
        assert result.returncode==expect,(argv,result.returncode,result.stdout,result.stderr)
        return json.loads(result.stdout)
    simulation=run(['pwsh','-NoProfile','-File',repo/'scripts/install-global-codex-execution.ps1','-Simulate','-WorkerPath',args.worker.resolve(),'-ClientPath',args.client.resolve()])
    root=Path(simulation['simulation_root'])
    project=root/'checkpoint-project';project.mkdir()
    project.joinpath('mixed.txt').write_bytes(b'LF\nCRLF\r\n'+b'archive-content\0'*14000)
    # Unmarked use is unchanged. The simulator then acts as the fixture owner;
    # production migration must provide its own verified backup/rollback route.
    initial=run([runtime,'init','--workspace',project])
    assert initial['ok']
    local=project/'.agent-checkpoints/status.sqlite3'
    runtime_sha=hashlib.sha256(runtime.read_bytes()).hexdigest()
    runtime_copy=project/'.agent-checkpoints/runtime'/f'save-work-status-{runtime_sha}.exe'
    runtime_copy.parent.mkdir()
    shutil.copyfile(runtime,runtime_copy)
    with database(local) as db:
        config=json.loads(db.execute('SELECT config_json FROM workspaces').fetchone()[0])
        config.update(runtime_exe=str(runtime_copy),rust_runtime_sha256=runtime_sha)
        db.execute('UPDATE workspaces SET config_json=?',(json.dumps(config),))
    original=hashlib.sha256(local.read_bytes()).hexdigest()
    enrolled=run([root/'worker.exe','--format','json','enroll','--root',root,'--workspace',project,'--formal-state','--yes'])
    registration=enrolled.get('registration',enrolled)
    worktree_id=registration['worktree_id']
    owned=root/f'checkpoint-{worktree_id}.sqlite3'
    prepared=run([root/'worker.exe','--format','json','prepare-checkpoint-migration','--root',root,'--workspace',project,'--runtime-sha256',runtime_sha,'--yes'])
    assert prepared['state']=='prepared' and prepared['routing_published'] is False
    assert prepared['backup_sha256']==original
    assert hashlib.sha256((root/prepared['backup_name']).read_bytes()).hexdigest()==original
    assert hashlib.sha256(local.read_bytes()).hexdigest()==original
    publication=run([root/'worker.exe','--format','json','publish-checkpoint-migration','--root',root,'--workspace',project,'--transaction-id',prepared['transaction_id'],'--yes'])
    assert publication['state']=='committed' and publication['routing_published']
    replay=run([root/'worker.exe','--format','json','publish-checkpoint-migration','--root',root,'--workspace',project,'--transaction-id',prepared['transaction_id'],'--yes'])
    assert replay['state']=='committed'
    state=project/'.agent-checkpoints'
    marker=state/'global-state.json'
    transition=state/'global-state-transition.json'
    journal=root/f"checkpoint-migration-{worktree_id}-{prepared['transaction_id']}.json"
    routing=root/f'checkpoint-routing-{worktree_id}.json'
    def write_json(path,value):
        path.write_text(json.dumps(value,sort_keys=True,indent=2,ensure_ascii=False),encoding='utf-8',newline='\r\n')
    def resume_publication():
        return run([root/'worker.exe','--format','json','publish-checkpoint-migration','--root',root,'--workspace',project,'--transaction-id',prepared['transaction_id'],'--yes'])
    transition_record={'schema':'rayman.checkpoint-transition.v1','transaction_id':prepared['transaction_id'],'registration_sha256':prepared['registration_sha256']}
    write_json(transition,transition_record)
    assert resume_publication()['state']=='committed' and not transition.exists()
    # Reconstruct exact crash frontiers from real published objects, keeping
    # their native file identities. Recovery must never regenerate a candidate.
    for missing_store in (False,True):
        marker.rename(state/f"global-state-{prepared['transaction_id']}.tmp")
        if missing_store:
            owned.rename(root/prepared['stage_name'])
            (state/f"global-preparing-{prepared['transaction_id']}.done").rename(marker)
        write_json(transition,transition_record)
        interrupted=json.loads(journal.read_text(encoding='utf-8'));interrupted['state']='publishing'
        write_json(journal,interrupted)
        write_json(routing,{'active':False,'transaction_id':prepared['transaction_id'],'registration_sha256':prepared['registration_sha256']})
        assert resume_publication()['state']=='committed' and not transition.exists()
    ready_marker=marker.read_bytes()
    env['RAYMAN_GLOBAL_EXECUTION_ROOT']=str(root)
    service=subprocess.Popen([str(root/'worker.exe'),'serve','--root',str(root)],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,creationflags=subprocess.CREATE_NO_WINDOW)
    try:
        protected_before=hashlib.sha256(owned.read_bytes()).hexdigest()
        marker_before=marker.read_bytes()
        refused=run([runtime,'pause','--workspace',project],expect=20)
        assert 'requires the global owner migration adapter' in refused['error']
        refused=run([runtime,'acknowledge','--workspace',project,'--notice-id','0'*64],expect=20)
        assert 'requires the global owner migration adapter' in refused['error']
        assert hashlib.sha256(owned.read_bytes()).hexdigest()==protected_before
        assert marker.read_bytes()==marker_before
        changed=run([runtime,'init','--workspace',project,'--interval-minutes','73'])
        assert changed['ok'] and changed['session']['interval_seconds']==4380,changed
        assert hashlib.sha256(local.read_bytes()).hexdigest()==original
        with database(owned) as db:
            assert db.execute('SELECT interval_seconds FROM sessions').fetchone()[0]==4380
            # A real SQLite writer conflict happens after local command execution.
            # No optimistic success may escape; candidate recovery files survive.
            db.execute('BEGIN IMMEDIATE')
            with ThreadPoolExecutor(max_workers=1) as pool:
                pending=pool.submit(run,[runtime,'init','--workspace',project,'--interval-minutes','99'],2)
                observed=[]
                deadline=time.monotonic()+8
                while time.monotonic()<deadline and not pending.done():
                    heartbeat=json.loads((root/'heartbeat.json').read_text(encoding='utf-8'))
                    observed.append(heartbeat)
                    time.sleep(.2)
                assert any(item['activity']['request_id'] is not None for item in observed),observed
                timestamps={item['observed_at'] for item in observed}
                assert len(timestamps)>=2,observed
                failed=pending.result(timeout=30)

            assert failed['ok'] is False and 'working state retained at' in failed['error'],failed
            assert db.execute('SELECT interval_seconds FROM sessions').fetchone()[0]==4380
            db.rollback()
        # Actual checkpoint capture and verification still run in SaveStatus.
        handoff={'objective':'Global checkpoint fixture','summary':'Real process data persistence','completed':['Initialize fixture'],'in_progress':['Verify adapter'],'next_steps':['Read back checkpoint'],'decisions':['Use isolated data'],'verification':[{'command':'fixture SQLite query','outcome':'passed','evidence':'4380 seconds observed'}],'known_issues':[],'active_processes':[],'recovery_files':[],'health':{'state':'healthy','rollback_detected':False,'error_active':False}}
        handoff_path=root/'handoff.json';handoff_path.write_text(json.dumps(handoff),encoding='utf-8',newline='\n')
        saved=run([runtime,'save','--workspace',project,'--handoff',handoff_path,'--force','--agent','codex'])
        assert saved['ok'],saved
        status=run([root/'client.exe','--format','json','status','--root',root])
        assert status['service_healthy'] and status['heartbeat_fresh'],status
        verified=run([runtime,'verify','--workspace',project])
        assert verified['ok'],verified
        assert hashlib.sha256(local.read_bytes()).hexdigest()==original
        with database(owned) as db:
            assert db.execute('SELECT count(*) FROM checkpoints').fetchone()[0]==1
        lease=run([root/'client.exe','app-state','--root',root,'--workspace',project,'--action','acquire','--object','checkpoints'])['lease_id']
        old_copy=root/'foreign-before.sqlite3';new_copy=root/'foreign-after.sqlite3'
        for path in (old_copy,new_copy):
            run([root/'client.exe','checkpoint-copy','--root',root,'--workspace',project,'--destination',path])
        with database(new_copy) as db:
            db.execute("INSERT INTO session_intents VALUES(?, '{}', 'now')",('b'*32,))
        rejected=subprocess.run([str(root/'client.exe'),'checkpoint-apply','--root',str(root),'--workspace',str(project),'--before',str(old_copy),'--after',str(new_copy),'--lease-id',lease,'--transaction-id','c'*32],env=env,capture_output=True,text=True,encoding='utf-8',timeout=30)
        assert rejected.returncode==1 and 'another tenant' in rejected.stderr,rejected.stderr
        run([root/'client.exe','app-state','--root',root,'--workspace',project,'--action','release','--lease-id',lease])
        rolled=run([root/'worker.exe','--format','json','rollback-checkpoint-migration','--root',root,'--workspace',project,'--transaction-id',prepared['transaction_id'],'--yes'])
        assert rolled['state']=='rolled_back' and not (project/'.agent-checkpoints/global-state.json').exists(),rolled
        local_verified=run([runtime,'verify','--workspace',project])
        assert local_verified['ok'],local_verified
        with database(local) as db:
            assert db.execute('SELECT count(*) FROM checkpoints').fetchone()[0]==1
            assert db.execute('SELECT interval_seconds FROM sessions').fetchone()[0]==4380
        assert owned.is_file() and (root/prepared['backup_name']).is_file()
        def resume_rollback():
            return run([root/'worker.exe','--format','json','rollback-checkpoint-migration','--root',root,'--workspace',project,'--transaction-id',prepared['transaction_id'],'--yes'])
        assert resume_rollback()['state']=='rolled_back'
        for frontier in ('local_published','original_moved','snapshot_ready'):
            snapshot=rolled.copy();snapshot['state']='rolling_back'
            rollback_stage=state/f"rollback-{prepared['transaction_id']}-{rolled['rollback_attempt']}.sqlite3"
            displaced=state/f"rollback-original-{prepared['transaction_id']}-{rolled['rollback_attempt']}.sqlite3"
            if frontier!='local_published':
                local.rename(rollback_stage)
                if frontier=='snapshot_ready':
                    displaced.rename(local)
            marker.write_bytes(ready_marker)
            write_json(journal,snapshot)
            assert resume_rollback()['state']=='rolled_back' and not marker.exists()
            assert run([runtime,'verify','--workspace',project])['ok']
        print(json.dumps({'ok':True,'rollback_preserved_latest_checkpoint':True,'publication_and_rollback_recovery':True,'owner_backup_preparation':True,'owner_migration_publication':True,'unsupported_workspace_control_refused_without_writes':True,'heartbeat_during_blocked_write':True,'foreign_copy_mutation_rejected':True,'simulation_root':str(root),'original_unchanged_until_rollback':True,'global_init_save_verify':True,'failed_persistence_withheld_success':True,'production_installed':False}))
    finally:
        service.terminate()
        service.wait(timeout=15)
        if service.stderr: service.stderr.close()

if __name__=='__main__':
    main()
