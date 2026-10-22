#!/usr/bin/env node
// Offline only. Validation keys are disposable; publisher keys require external
// approval + independently configured public fingerprint. Never publishes/trusts.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import {signRelease} from './sign-release.mjs';

function run(args){
  if(args.length%2)throw new Error('options');
  const flags=new Map();
  for(let i=0;i<args.length;i+=2){if(!['--manifest','--product','--output-dir','--mode'].includes(args[i])||flags.has(args[i]))throw new Error('options');flags.set(args[i],args[i+1]);}
  const product=flags.get('--product');const manifest=flags.get('--manifest');const output=flags.get('--output-dir');const mode=flags.get('--mode')??'fixture';
  if(!manifest||!output||!['shell','runtime'].includes(product)||!['fixture','publisher'].includes(mode))throw new Error('options');
  let keyId='validation-only';let pem;let expected;
  if(mode==='publisher'){
    if(process.env.OCEAN_RELEASE_DRAFT_APPROVED!=='yes')throw new Error('approval');
    keyId=process.env.OCEAN_RELEASE_KEY_ID;
    expected=process.env.OCEAN_RELEASE_PUBLIC_KEY_SHA256;
    const secret=process.env.OCEAN_RELEASE_SIGNING_KEY_PEM;
    if(!keyId||!expected||!/^[a-f0-9]{64}$/.test(expected)||!secret||Buffer.byteLength(secret)>8192)throw new Error('inputs');
    pem=Buffer.from(secret);delete process.env.OCEAN_RELEASE_SIGNING_KEY_PEM;
  }else{
    // TEST VALIDATION ONLY: never export this private material or install trust.
    pem=Buffer.from(crypto.generateKeyPairSync('ed25519').privateKey.export({format:'pem',type:'pkcs8'}));
  }
  const scratch=fs.mkdtempSync(path.join(os.tmpdir(),'ocean-release-key-'));fs.chmodSync(scratch,0o700);const identity=fs.lstatSync(scratch);
  try{
    const key=path.join(scratch,'key.pem');fs.writeFileSync(key,pem,{flag:'wx',mode:0o600});
    const signed=signRelease(['--manifest',manifest,'--private-key',key,'--key-id',keyId,'--product',product]);
    if(mode==='publisher'&&signed.publicKeySha256!==expected)throw new Error('fingerprint');
    fs.mkdirSync(output,{mode:0o700});
    fs.writeFileSync(path.join(output,'signature.json'),JSON.stringify(signed.signature)+'\n',{flag:'wx',mode:0o644});
    fs.writeFileSync(path.join(output,'public-key.json'),JSON.stringify(signed.publicKey)+'\n',{flag:'wx',mode:0o644});
    return {validationOnly:mode!=='publisher',product,keyId,manifestSha256:signed.manifestSha256,publicKeySha256:signed.publicKeySha256,warning:'Offline signature only. Run independent Rust policy/artifact/native checks. No trust, release or update was installed.'};
  }finally{
    pem?.fill(0);
    const actual=fs.lstatSync(scratch);
    if(!actual.isDirectory()||actual.isSymbolicLink()||actual.dev!==identity.dev||actual.ino!==identity.ino||fs.realpathSync(scratch)!==scratch)throw new Error('scratch');
    fs.rmSync(scratch,{recursive:true});
  }
}
process.stdout.on('error',()=>{process.exitCode=1;});process.stderr.on('error',()=>{process.exitCode=1;});
try{process.stdout.write(JSON.stringify(run(process.argv.slice(2)))+'\n');}
catch{process.stderr.write('publisher signature refused: check explicit product/mode, protected approval and independent key fingerprint; no publication performed\n');process.exitCode=1;}
