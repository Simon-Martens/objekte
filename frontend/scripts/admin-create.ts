import { input, password } from '@inquirer/prompts';
import { stdin } from 'node:process';
import { getAuth } from '../src/server/auth';

const auth = getAuth();
let email:string, displayName:string, passphrase:string;
if (stdin.isTTY) {
  email=(await input({message:'Email address'})).trim();
  displayName=(await input({message:'Display name'})).trim();
  passphrase=await password({message:'Password (at least 12 characters)',mask:'*'});
} else {
  let text=''; for await (const chunk of stdin) text+=chunk;
  [email,displayName,passphrase]=text.split(/\r?\n/);
  if (!email || !displayName || !passphrase) throw new Error('pipe email address, display name, and password on separate lines');
}
await auth.bootstrap(email,displayName,passphrase);
console.log('Administrator created.');
