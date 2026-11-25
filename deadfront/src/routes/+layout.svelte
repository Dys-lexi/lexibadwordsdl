<script lang="ts">
	import type { LayoutProps } from './$types';
	import Header from './Header.svelte';
	import './layout.css';

	let { children }: LayoutProps = $props();
</script>


	<main>
		{@render children()}
	</main>

