<script lang="ts">
  import FolderProvisioning from './FolderProvisioning.svelte'
  let { account, lang, disabled, folders, onpreview }: { account: string; lang: 'en' | 'de'; disabled: boolean; folders: boolean; onpreview: () => void } = $props()
  let selected = $state(false)
</script>
<div class="archive-offer">
  <label><input type="checkbox" bind:checked={selected} {disabled} />{lang === 'en' ? 'Review archiving old unmatched Inbox mail separately' : 'Archivierung alter unzugeordneter Posteingangsnachrichten separat prüfen'}</label>
  <p class="help">{lang === 'en' ? 'Only mail older than twelve calendar months by arrival date qualifies. Review holds stay in Inbox. Flags and unrelated Gmail labels remain unchanged. Nothing is included in ordinary sorting.' : 'Nur Nachrichten mit einem Eingangsdatum vor mehr als zwölf Kalendermonaten kommen infrage. Zur Prüfung zurückgehaltene Nachrichten bleiben im Posteingang. Flags und andere Gmail-Labels bleiben erhalten. Die normale Sortierung umfasst diese Archivierung nicht.'}</p>
  <button disabled={disabled || !selected} onclick={onpreview}>{lang === 'en' ? 'Preview old unmatched mail for archiving' : 'Alte unzugeordnete Nachrichten zur Archivierung anzeigen'}</button>
  {#if selected && folders}<FolderProvisioning {account} {lang} intent="archive_unmatched" />{/if}
</div>
