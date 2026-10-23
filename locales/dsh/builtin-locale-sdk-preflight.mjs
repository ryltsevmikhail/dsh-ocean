// Owned-runtime metadata subprocess only. Never boots Loader or imports row code.
import {statSync,realpathSync,openSync,fstatSync,readSync,closeSync,constants} from 'node:fs';
import {dirname,join,isAbsolute} from 'node:path';
import {createRequire} from 'node:module';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {inspectConfigured} from './builtin-locale-preflight.mjs';
function json(path){const fd=openSync(path,constants.O_RDONLY|constants.O_NOFOLLOW|constants.O_NONBLOCK);try{const stat=fstatSync(fd);if(!stat.isFile()||stat.size>1048576)throw new Error('manifest bound');const bytes=Buffer.alloc(1048577);let length=0;while(length<bytes.length){const count=readSync(fd,bytes,length,bytes.length-length,null);if(!count)break;length+=count;}if(length>1048576)throw new Error('manifest bound');return JSON.parse(bytes.subarray(0,length).toString('utf8'));}finally{closeSync(fd);}}
function missing(path){try{statSync(path);return false;}catch(error){if(error.code==='ENOENT')return true;throw error;}}
export async function disposeMetadataContext(context){await context.fiber.dispose();}
export function owningManifest(moduleURL){
 if(!moduleURL.startsWith('file:'))return undefined;
 let directory=dirname(realpathSync(fileURLToPath(moduleURL)));
 for(let depth=0;depth<64;depth++){
  const manifest=join(directory,'package.json');
  if(!missing(manifest)){const data=json(manifest);return typeof data.name==='string'?data:undefined;}
  const parent=dirname(directory);if(parent===directory)return undefined;directory=parent;
 }
}
export function readOnlyProfile(sdk,profileDir,installAnchor){
 if(!missing(join(profileDir,'package.json')))return sdk.loadProfileDirectory('ocean-locales',profileDir,installAnchor);
 const layers=[];
 for(const packageName of sdk.PROFILE_TEMPLATES.web.bundles){
  const packageDir=sdk.resolveBundleDir('ocean-locales',packageName,installAnchor,profileDir);
  const bundle=sdk.readProfileManifest('ocean-locales',packageDir).dsh?.bundle;
  if(!bundle)throw new Error('missing shipped bundle');
  const patchPaths=sdk.bundlePatchPaths(packageDir,bundle);
  layers.push({packageName,packageDir,patchPaths,patches:patchPaths.flatMap(path=>sdk.loadOverlayPatches('ocean-locales',path))});
 }
 const patchPath=join(profileDir,'cordis.patch.yml');
 return {name:'web',dir:profileDir,layers,patchPath,patches:sdk.loadOptionalPatches('ocean-locales',patchPath)??[],skippedBundles:[]};
}
export async function preflight({installAnchor,profileDir,home,cwd,telemetryDisabledEnv}){
 if(![installAnchor,profileDir,home,cwd].every(path=>typeof path==='string'&&isAbsolute(path)&&!path.split('/').includes('..')))return {state:'unknown'};
 let context;
 try{
  const require=createRequire(installAnchor);
  const sdk=await import(pathToFileURL(require.resolve('@deepseek-ai/dsh-app-boot')).href);
  const {Context}=await import(pathToFileURL(require.resolve('@deepseek-ai/cordis')).href);
  const profile=readOnlyProfile(sdk,profileDir,installAnchor);
  context=new Context();let packages;
  return await inspectConfigured({sdk,profile,installAnchor,home,cwd,telemetryDisabledEnv,resolveIdentity(name,{resolution}){
   if(name.startsWith('file:'))return owningManifest(name);
   if(name.startsWith('.')||isAbsolute(name)||name.includes(':'))return undefined;
   packages??=new sdk.PluginPackages(context,{resolution});
   // Public Node resolution, with the SDK's public metadata service hooks.
   // The explicit-parent form requires the documented Node CLI flag; without
   // it Node ignores the profile base, so never guess or use ambient lookup.
   if(!process.execArgv.includes('--experimental-import-meta-resolve'))return undefined;
   return owningManifest(import.meta.resolve(name,pathToFileURL(profileDir+'/').href));
  }});
 }catch{return {state:'unknown'};}
 finally{if(context)await disposeMetadataContext(context);}
}
// Explicit CLI entry; imports by tests never discover a runtime or home.
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href){
 let result={state:'unknown'};
 try{result=await preflight(JSON.parse(process.argv[2]));}catch{}
 process.stdout.write(JSON.stringify(result)+'\n');
}
