@extends('layouts.app')
@include('partials.header')
@include  ('spaced')
@include('partials.card', [
    'title' => $title,
    'items' => $items,
])
<form method="POST">
@csrf
@method('PUT')
</form>
@vite(['resources/css/app.css', 'resources/js/app.js'])
@json($data, JSON_PRETTY_PRINT)
@dd($x)
@yield('content')
@stack('scripts')
@includeWhen($cond, 'view', ['a' => 1])
@each('view.name', $jobs, 'job', 'view.empty')
@inject('metrics', 'App\Services\MetricsService')
@props([
    'type' => 'info',
    'message',
])
@csrf   @method('DELETE')
