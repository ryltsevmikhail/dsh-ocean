// Generated resources are checked in: cargo builds need no Node, network or add-on.
import {readFileSync,writeFileSync,mkdirSync,readdirSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {resolve,dirname,relative} from 'node:path';
import {languages} from '../language-pack/src/register.mjs';
import {validateCatalogs} from '../language-pack/src/validate.mjs';
import {renderClient} from '../language-pack/scripts/build.mjs';
const root=fileURLToPath(new URL('../',import.meta.url));
const destination=resolve(root,'locales/dsh');
const check=process.argv.slice(2).includes('--check');
if(process.argv.slice(2).some(x=>x!=='--check'))throw new Error('only --check is supported');
const catalogs=Object.fromEntries(languages.map(({id})=>[id,JSON.parse(readFileSync(resolve(root,'language-pack/data',id+'.json'),'utf8'))]));
validateCatalogs(catalogs,JSON.parse(readFileSync(resolve(root,'language-pack/data/en.json'),'utf8')));
const manifest={
 name:'@dsh-ocean/builtin-locales',version:'0.4.0',private:true,type:'module',license:'MIT',
 description:'Built-in DSH Ocean UI dictionaries; application-owned composition only',
 main:'./lib/index.js',exports:{'.':'./lib/index.js','./client':'./lib/client.js','./package.json':'./package.json'},
 dsh:{client:{platform:'web',immediately:true,inject:['@deepseek-ai/dsh-client-locale']}},
 engines:{dsh:'0.2.0-rc.2',node:'>=22'},
 peerDependencies:{'@deepseek-ai/cordis':'~4.0.4','@deepseek-ai/dsh-client-locale':'0.2.0-rc.2'},
};
const files={
 'package.json':JSON.stringify(manifest,null,2)+'\n',
 'lib/index.js':'/** Built-in host entry: no settings, dependency or profile writes. */\nexport function apply() {}\n',
 'lib/client.js':renderClient(catalogs,manifest.name),
 'overlay.yml':'# Application-owned activation; native profile consent is required.\n- insert:\n    - id: ocean-builtin-locales\n      name: ./lib/index.js\n',
 LICENSE:readFileSync(resolve(root,'language-pack/LICENSE'),'utf8'),
 'builtin-locale-preflight.mjs':readFileSync(resolve(root,'scripts/builtin-locale-preflight.mjs'),'utf8'),
 'builtin-locale-sdk-preflight.mjs':readFileSync(resolve(root,'scripts/builtin-locale-sdk-preflight.mjs'),'utf8'),
};
for(const [name,expected] of Object.entries(files)){
 const path=resolve(destination,name);
 if(check){if(readFileSync(path,'utf8')!==expected)throw new Error('stale built-in locale resource: '+name);}
 else {mkdirSync(dirname(path),{recursive:true});writeFileSync(path,expected);}
}
function inventory(path){return readdirSync(path,{withFileTypes:true}).flatMap(entry=>{
 if(entry.isSymbolicLink())throw new Error('built-in resource cannot be a symlink');
 const child=resolve(path,entry.name);return entry.isDirectory()?inventory(child):[relative(destination,child)];
});}
if(JSON.stringify(inventory(destination).sort())!==JSON.stringify(Object.keys(files).sort()))throw new Error('unexpected built-in resource files');
console.log(`${check?'Checked':'Generated'} ${languages.length} built-in DSH locales (${Object.keys(files).length} resource files)`);
